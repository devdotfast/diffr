//! Projection from the internal `DiffResult` onto the wire types.
//!
//! Leaves come from the full-file row alignment: consecutive rows of one
//! kind (paired unchanged, paired novel, one-sided) become one leaf on each
//! side they touch, and a paired leaf shares its `alignment_id` across
//! sides, so zipping leaves by `alignment_id` gives the rows. Folds come
//! from the per-side fold lists and carry no `alignment_id`. A fold whose
//! partner (see `folds::partner`) is also a region is a matched pair, and
//! the two share `fold_state_id`: they open and close together.
//!
//! A fold covers its body alone: `folds::line_span` rounds its byte range in
//! to whole lines, not out, so the header line it opens on — and the line its
//! `}` sits on — belong to the leaves beside it. A fold's lines are then
//! exactly the lines collapsing it hides, and a fold starting a line after
//! the construct it belongs to is a different region from the scope around
//! it.
//!
//! Numbering (see `Ids`) runs as the regions are built, lhs preorder then
//! rhs preorder, with two counters: `id` dense from 1, above [`ROOT`],
//! and `alignment_id` dense from 0. Every region takes the next `id`, on
//! either side, so no id is shared. Every leaf that is not
//! the second of a pair takes the next `alignment_id`; the second takes its
//! counterpart's. A region's `fold_state_id` is its own `id`, except that the
//! second of a paired leaf or a matched fold takes its counterpart's
//! `fold_state_id`. Leaves are split wherever a
//! fold starts or ends so that every fold's children tile its line span
//! exactly, and a split on one side of a paired leaf is mirrored on the
//! other so paired leaves stay equal in length. Nothing starts collapsed:
//! which unchanged lines to hide is the `context` plugin's.
use super::{
    BinaryRef, Diff, FileRef, LineCounts, Node, Problem, Region, Source, SourcePos, SourceRange,
    Span, Stats, SyntaxSpan, Visibility, ROOT,
};
use crate::hash::DftHashMap;
use crate::line_layout::{aligned_rows, novel_lines, runs, Run};
use crate::line_parser;
use crate::pairing::Pairing;
use crate::parse::folds::{self, Fold, FoldMatch};
use crate::parse::syntax::{MatchKind, MatchedPos, SyntaxId};
use crate::parse::tree_sitter_parser::{highlight_captures, TreeSitterConfig};
use crate::summary::{DiffResult, FallbackCause, FileContent, FileFormat};
use std::collections::{BTreeMap, BTreeSet};

/// Everything the projection needs besides the diff itself.
pub struct Inputs<'a> {
    /// Which sides the file exists on; a one-sided file gets one source.
    pub file: &'a Pairing<FileRef>,
    /// Byte length of each side's content, for binary files.
    pub sizes: (u64, u64),
    /// Highlight spans per side; empty when the run did not ask for syntax.
    pub syntax: (Vec<SyntaxSpan>, Vec<SyntaxSpan>),
}

pub fn diff(result: &DiffResult, inputs: Inputs<'_>) -> Diff {
    let (lhs_src, rhs_src) = match (&result.lhs_src, &result.rhs_src) {
        (FileContent::Text(lhs), FileContent::Text(rhs)) => (lhs.as_str(), rhs.as_str()),
        _ => {
            let sides = pair(
                inputs.file,
                BinaryRef {
                    size: inputs.sizes.0,
                },
                BinaryRef {
                    size: inputs.sizes.1,
                },
            );
            return Diff::Binary { sides };
        }
    };
    let (lhs_regions, rhs_regions) = regions(result, lhs_src, rhs_src);
    let (lhs_syntax, rhs_syntax) = inputs.syntax;
    let sides = pair(
        inputs.file,
        Source {
            text: lhs_src.to_owned(),
            syntax: lhs_syntax,
            regions: lhs_regions,
        },
        Source {
            text: rhs_src.to_owned(),
            syntax: rhs_syntax,
            regions: rhs_regions,
        },
    );
    Diff::Text {
        sides,
        stats: stats(result, lhs_src, rhs_src),
        // Filled together with visible counts after plugins shape the trees.
        structural_changes: Default::default(),
    }
}

fn pair<T>(file: &Pairing<FileRef>, lhs: T, rhs: T) -> Pairing<T> {
    match file {
        Pairing::Both { .. } => Pairing::Both { lhs, rhs },
        Pairing::LeftOnly { .. } => Pairing::LeftOnly { lhs },
        Pairing::RightOnly { .. } => Pairing::RightOnly { rhs },
    }
}

fn stats(result: &DiffResult, lhs_src: &str, rhs_src: &str) -> Stats {
    let (lhs_lines, rhs_lines) = line_parser::change_positions(lhs_src, rhs_src);
    let textual = LineCounts {
        added: novel_lines(&rhs_lines).len() as u32,
        removed: novel_lines(&lhs_lines).len() as u32,
    };
    let fallback = match &result.file_format {
        FileFormat::SupportedLanguage(_) => None,
        FileFormat::PlainText => Some(Problem {
            code: "unsupported_language".to_owned(),
            message: "no tree-sitter grammar for this file".to_owned(),
        }),
        FileFormat::TextFallback { cause, reason } => Some(Problem {
            code: fallback_code(*cause).to_owned(),
            message: reason.clone(),
        }),
        FileFormat::Binary => unreachable!("binary files never reach text stats"),
    };
    Stats {
        textual,
        // Before any plugin runs nothing starts collapsed; the stream
        // recounts after plugins.
        visible: textual,
        fallback,
    }
}

/// The wire code for why a file was diffed by line. The message beside it
/// is the engine's own prose, with the numbers.
fn fallback_code(cause: FallbackCause) -> &'static str {
    match cause {
        FallbackCause::Generated => "generated",
        FallbackCause::ByteLimit => "too_large",
        FallbackCause::GraphLimit => "too_complex",
        FallbackCause::ParseErrorLimit => "parse_error",
    }
}

/// Highlight spans for one side, per line, sorted, non-overlapping. Where
/// captures nest the innermost wins.
pub fn syntax_spans(src: &str, parser: &'static TreeSitterConfig) -> Vec<SyntaxSpan> {
    let mut captures = highlight_captures(src, parser);
    // Paint larger captures first so smaller (inner) ones overwrite them.
    captures.sort_by_key(|(start, end, _)| std::cmp::Reverse(end - start));
    let mut owner: Vec<Option<&'static str>> = vec![None; src.len()];
    for (start, end, name) in captures {
        for slot in &mut owner[start..end] {
            *slot = Some(name);
        }
    }
    let mut spans = Vec::new();
    let mut line_start = 0;
    for (line, text) in src.split_inclusive('\n').enumerate() {
        let content_len = text.trim_end_matches('\n').len();
        let mut run: Option<(usize, &'static str)> = None;
        for column in 0..=content_len {
            let current = (column < content_len)
                .then(|| owner[line_start + column])
                .flatten();
            match (run, current) {
                (Some((_, name)), Some(now)) if now == name => {}
                (Some((start, name)), _) => {
                    spans.push(SyntaxSpan {
                        line: line as u32,
                        start_column: start as u32,
                        end_column: column as u32,
                        capture: name.to_owned(),
                    });
                    run = current.map(|name| (column, name));
                }
                (None, Some(name)) => run = Some((column, name)),
                (None, None) => {}
            }
        }
        line_start += text.len();
    }
    spans
}

// ── regions ───────────────────────────────────────────────────────────────

/// A leaf after splitting. `key` identifies its counterpart on the other
/// side, when it has one.
#[derive(Clone, Debug)]
struct Leaf {
    lines: (usize, usize),
    key: Option<LeafKey>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct LeafKey {
    run: usize,
    piece: usize,
}

struct SideFold<'a> {
    fold: &'a Fold,
    lines: (usize, usize),
}

fn regions(result: &DiffResult, lhs_src: &str, rhs_src: &str) -> (Vec<Region>, Vec<Region>) {
    let lhs_lines: Vec<&str> = lhs_src.split_terminator('\n').collect();
    let rhs_lines: Vec<&str> = rhs_src.split_terminator('\n').collect();
    let lhs_novel = novel_lines(&result.lhs_positions);
    let rhs_novel = novel_lines(&result.rhs_positions);
    let sources = (lhs_src, rhs_src);
    let positions = (
        result.lhs_positions.as_slice(),
        result.rhs_positions.as_slice(),
    );
    let rows = aligned_rows(sources, positions);
    let runs = runs(&rows, &lhs_novel, &rhs_novel);
    let lhs_folds = side_folds(&result.lhs_folds, &lhs_lines);
    let rhs_folds = side_folds(&result.rhs_folds, &rhs_lines);
    let lhs_splits = folds::split_lines(lhs_folds.iter().map(|fold| fold.lines));
    let rhs_splits = folds::split_lines(rhs_folds.iter().map(|fold| fold.lines));
    let (lhs_leaves, rhs_leaves) = split_runs(&runs, &lhs_splits, &rhs_splits);

    let mut ids = Ids::new();
    let lhs = tree(
        &lhs_folds,
        &lhs_leaves,
        &result.lhs_positions,
        &lhs_novel,
        &lhs_lines,
        &mut ids,
    );
    let rhs = tree(
        &rhs_folds,
        &rhs_leaves,
        &result.rhs_positions,
        &rhs_novel,
        &rhs_lines,
        &mut ids,
    );
    (lhs, rhs)
}

/// One side's folds as nested line spans.
///
/// A fold dropped here is never numbered, so its partner on the other side
/// takes an id of its own (see `Ids::fold`).
fn side_folds<'a>(side: &'a [Fold], lines: &[&str]) -> Vec<SideFold<'a>> {
    let spans: Vec<(usize, usize)> = side
        .iter()
        .map(|fold| folds::line_span(fold, lines))
        .collect();
    side.iter()
        .zip(folds::nested_spans(&spans))
        // A fold on a single line hides nothing; it is not a region.
        .filter_map(|(fold, span)| Some(SideFold { fold, lines: span? }))
        .collect()
}

/// Split every run at its side's fold boundaries, mirroring splits across
/// paired runs so both sides keep equal-length pieces.
fn split_runs(
    runs: &[Run],
    lhs_splits: &BTreeSet<usize>,
    rhs_splits: &BTreeSet<usize>,
) -> (Vec<Leaf>, Vec<Leaf>) {
    let mut lhs_leaves = Vec::new();
    let mut rhs_leaves = Vec::new();
    for (index, run) in runs.iter().enumerate() {
        let len = run.len();
        let (lhs, rhs, paired) = match run.sides {
            Pairing::Both { lhs, rhs } => (Some(lhs), Some(rhs), true),
            Pairing::LeftOnly { lhs } => (Some(lhs), None, false),
            Pairing::RightOnly { rhs } => (None, Some(rhs), false),
        };
        let mut offsets: BTreeSet<usize> = BTreeSet::new();
        if let Some((start, end)) = lhs {
            offsets.extend(lhs_splits.range(start + 1..end).map(|line| line - start));
        }
        if let Some((start, end)) = rhs {
            offsets.extend(rhs_splits.range(start + 1..end).map(|line| line - start));
        }
        let mut at = 0;
        for (piece, cut) in offsets.into_iter().chain([len]).enumerate() {
            let key = paired.then_some(LeafKey { run: index, piece });
            if let Some((start, _)) = lhs {
                lhs_leaves.push(Leaf {
                    lines: (start + at, start + cut),
                    key,
                });
            }
            if let Some((start, _)) = rhs {
                rhs_leaves.push(Leaf {
                    lines: (start + at, start + cut),
                    key,
                });
            }
            at = cut;
        }
    }
    (lhs_leaves, rhs_leaves)
}

/// Wire id allocation, in the order regions are built: lhs preorder, then
/// rhs preorder. `id` and leaf `alignment_id` are separate counters: `id`
/// dense from 1, since [`ROOT`] names the file, and `alignment_id` from 0. Every region takes a fresh `id`. A leaf takes a fresh
/// `alignment_id` and its own `id` as `fold_state_id`, unless its
/// counterpart is already numbered, whose pair it then shares.
struct Ids {
    next_id: u32,
    next_alignment: u32,
    /// The `(alignment_id, fold_state_id)` of every paired leaf numbered so
    /// far, by its `LeafKey`.
    leaves: DftHashMap<LeafKey, (u32, u32)>,
    /// The `fold_state_id` of every fold numbered so far, by the fold of the
    /// syntax node it was built on.
    folds: DftHashMap<SyntaxId, u32>,
}

impl Ids {
    fn new() -> Self {
        Self {
            next_id: ROOT + 1,
            next_alignment: 0,
            leaves: DftHashMap::default(),
            folds: DftHashMap::default(),
        }
    }

    fn fresh_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    fn fresh_alignment(&mut self) -> u32 {
        let alignment = self.next_alignment;
        self.next_alignment += 1;
        alignment
    }

    /// A leaf's `(id, alignment_id, fold_state_id)`.
    fn leaf(&mut self, key: Option<LeafKey>) -> (u32, u32, u32) {
        let id = self.fresh_id();
        let Some(key) = key else {
            return (id, self.fresh_alignment(), id);
        };
        if let Some(&(alignment, state)) = self.leaves.get(&key) {
            return (id, alignment, state);
        }
        let alignment = self.fresh_alignment();
        self.leaves.insert(key, (alignment, id));
        (id, alignment, id)
    }

    /// A fold's `(id, fold_state_id)`. A fold whose opposite is already
    /// numbered shares the opposite's `fold_state_id`. An opposite that is
    /// never numbered, because its fold was dropped from its side, leaves
    /// the survivor a `fold_state_id` of its own.
    fn fold(&mut self, fold: &Fold) -> (u32, u32) {
        let id = self.fresh_id();
        let numbered = match fold.match_kind {
            FoldMatch::Matched { opposite } => self.folds.get(&opposite),
            FoldMatch::Novel => None,
        };
        let state = match numbered {
            Some(&state) => state,
            None => id,
        };
        self.folds.insert(fold.syntax_id, state);
        (id, state)
    }
}

enum Item<'a> {
    Fold(&'a SideFold<'a>),
    Leaf(&'a Leaf),
}

impl Item<'_> {
    fn lines(&self) -> (usize, usize) {
        match self {
            Self::Fold(fold) => fold.lines,
            Self::Leaf(leaf) => leaf.lines,
        }
    }

    /// Outer before inner: earlier start, then later end, then folds before
    /// leaves, then the wider byte range.
    fn order(
        &self,
    ) -> (
        usize,
        std::cmp::Reverse<usize>,
        u8,
        (usize, usize),
        std::cmp::Reverse<(usize, usize)>,
    ) {
        let (start, end) = self.lines();
        match self {
            Self::Fold(fold) => (
                start,
                std::cmp::Reverse(end),
                0,
                (
                    fold.fold.range.start.line.as_usize(),
                    fold.fold.range.start.byte_column,
                ),
                std::cmp::Reverse((
                    fold.fold.range.end.line.as_usize(),
                    fold.fold.range.end.byte_column,
                )),
            ),
            Self::Leaf(_) => (
                start,
                std::cmp::Reverse(end),
                1,
                (0, 0),
                std::cmp::Reverse((0, 0)),
            ),
        }
    }
}

/// Nest folds and leaves by containment on line spans and number them in
/// document order, outer before inner.
fn tree(
    folds: &[SideFold<'_>],
    leaves: &[Leaf],
    positions: &[MatchedPos],
    novel: &BTreeSet<usize>,
    lines: &[&str],
    ids: &mut Ids,
) -> Vec<Region> {
    let mut items: Vec<Item<'_>> = folds
        .iter()
        .map(Item::Fold)
        .chain(leaves.iter().map(Item::Leaf))
        .collect();
    items.sort_by_key(Item::order);
    let by_line = positions_by_line(positions);

    // Build bottom-up with an explicit stack of open folds.
    struct Open<'a> {
        fold: &'a SideFold<'a>,
        ids: (u32, u32),
        children: Vec<Region>,
    }
    let mut root: Vec<Region> = Vec::new();
    let mut stack: Vec<Open<'_>> = Vec::new();
    let close = |stack: &mut Vec<Open<'_>>, root: &mut Vec<Region>| {
        let open = stack.pop().expect("closing an open fold");
        let fold = open.fold.fold;
        // The wire range is the hull of the children, which tile whole
        // lines; the parser's byte columns inside the header line are not
        // carried, since nothing narrower than a line can be hidden.
        let (Some(first), Some(last)) = (open.children.first(), open.children.last()) else {
            return;
        };
        let range = SourceRange {
            start: first.range.start,
            end: last.range.end,
        };
        let region = Region {
            id: open.ids.0,
            fold_state_id: open.ids.1,
            range,
            tags: fold.tags.clone(),
            visibility: Visibility {
                collapsed: false,
                label: fold.placeholder.clone(),
            },
            node: Node::Fold {
                children: open.children,
            },
        };
        match stack.last_mut() {
            Some(parent) => parent.children.push(region),
            None => root.push(region),
        }
    };
    for item in items {
        let (start, _) = item.lines();
        while stack.last().is_some_and(|open| open.fold.lines.1 <= start) {
            close(&mut stack, &mut root);
        }
        match item {
            Item::Fold(fold) => {
                let fold_ids = ids.fold(fold.fold);
                stack.push(Open {
                    fold,
                    ids: fold_ids,
                    children: Vec::new(),
                });
            }
            Item::Leaf(leaf) => {
                if leaf.lines.0 == leaf.lines.1 {
                    continue;
                }
                let region = leaf_region(leaf, ids, &by_line, novel, lines);
                match stack.last_mut() {
                    Some(parent) => parent.children.push(region),
                    None => root.push(region),
                }
            }
        }
    }
    while !stack.is_empty() {
        close(&mut stack, &mut root);
    }
    root
}

fn leaf_region(
    leaf: &Leaf,
    ids: &mut Ids,
    by_line: &BTreeMap<usize, Vec<&MatchedPos>>,
    novel: &BTreeSet<usize>,
    lines: &[&str],
) -> Region {
    let (start, end) = leaf.lines;
    let mut changed = Vec::new();
    for line in novel.range(start..end) {
        let tokens = by_line.get(line).map(Vec::as_slice).unwrap_or(&[]);
        let all_novel = tokens.iter().all(|token| {
            matches!(
                token.kind,
                MatchKind::Novel { .. } | MatchKind::NovelWord { .. }
            )
        });
        if all_novel {
            // A blank novel line has nothing to paint.
            if !lines[*line].is_empty() {
                changed.push(Span {
                    line: *line as u32,
                    start_column: 0,
                    end_column: lines[*line].len() as u32,
                });
            }
            continue;
        }
        for token in tokens {
            if !matches!(
                token.kind,
                MatchKind::Novel { .. } | MatchKind::NovelWord { .. }
            ) {
                continue;
            }
            let span = Span {
                line: *line as u32,
                start_column: token.pos.start_col,
                end_column: token.pos.end_col,
            };
            match changed.last_mut() {
                Some(last) if last.line == span.line && last.end_column == span.start_column => {
                    last.end_column = span.end_column;
                }
                _ => changed.push(span),
            }
        }
    }
    let (id, alignment_id, fold_state_id) = ids.leaf(leaf.key);
    Region {
        id,
        fold_state_id,
        range: SourceRange {
            start: SourcePos {
                line: start as u32,
                column: 0,
            },
            end: SourcePos {
                line: end as u32,
                column: 0,
            },
        },
        tags: Vec::new(),
        visibility: Visibility::default(),
        node: Node::Leaf {
            alignment_id,
            changed,
        },
    }
}

fn positions_by_line(positions: &[MatchedPos]) -> BTreeMap<usize, Vec<&MatchedPos>> {
    let mut by_line: BTreeMap<usize, Vec<&MatchedPos>> = BTreeMap::new();
    for position in positions {
        by_line
            .entry(position.pos.line.as_usize())
            .or_default()
            .push(position);
    }
    for tokens in by_line.values_mut() {
        tokens.sort_by_key(|token| token.pos.start_col);
    }
    by_line
}
