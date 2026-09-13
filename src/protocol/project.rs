//! Projection from the internal `DiffResult` onto the wire types.
//!
//! Leaves come from the full-file row alignment: consecutive rows of one
//! kind (paired unchanged, paired novel, one-sided) become one leaf on each
//! side they touch, and a paired leaf shares its id across sides. Folds
//! come from the per-side fold lists; a fold whose partner (see
//! `folds::partner`) is also a region shares its id. Ids are numbered
//! densely as the regions are built, lhs first. Leaves are split wherever a
//! fold starts or ends so that every fold's children tile its line span
//! exactly, and a split on one side of a paired leaf is mirrored on the
//! other so paired leaves stay equal in length. Unchanged rows that no hunk
//! shows, which is everything outside the `-U` padding and the enclosing
//! syntax context difftastic already selected, become collapsed leaves
//! tagged `unchanged`. Folds that lie entirely inside such a gap are
//! dropped: they would be hidden anyway, and keeping them would only
//! fragment the gap.
use super::{
    BinaryRef, Diff, FileRef, LineCounts, Node, Problem, Region, Source, SourcePos, SourceRange,
    Span, Stats, SyntaxSpan, Visibility,
};
use crate::display::line_layout::{
    aligned_rows, novel_lines, runs, shown_lines, trim_context, Run, MIN_GAP,
};
use crate::hash::DftHashMap;
use crate::line_folds;
use crate::line_parser;
use crate::pairing::Pairing;
use crate::parse::folds::{self, Fold};
use crate::parse::syntax::{MatchKind, MatchedPos, SyntaxId};
use crate::parse::tree_sitter_parser::{highlight_captures, TreeSitterConfig};
use crate::summary::{DiffResult, FileContent, FileFormat};
use std::collections::{BTreeMap, BTreeSet};

/// Everything the projection needs besides the diff itself.
pub(crate) struct Inputs<'a> {
    /// Which sides the file exists on; a one-sided file gets one source.
    pub(crate) file: &'a Pairing<FileRef>,
    /// Byte length of each side's content, for binary files.
    pub(crate) sizes: (u64, u64),
    /// Highlight spans per side; empty when the run did not ask for syntax.
    pub(crate) syntax: (Vec<SyntaxSpan>, Vec<SyntaxSpan>),
}

pub(crate) fn diff(result: &DiffResult, inputs: Inputs<'_>) -> Diff {
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
        FileFormat::TextFallback { reason } => Some(Problem {
            code: fallback_code(reason).to_owned(),
            message: reason.clone(),
        }),
        FileFormat::Binary => unreachable!("binary files never reach text stats"),
    };
    Stats {
        textual,
        // Before any mutation runs nothing starts collapsed except context
        // gaps, which hold no changed lines; the stream recounts after
        // mutations.
        visible: textual,
        fallback,
    }
}

/// difftastic reports its fallbacks as prose; the wire wants a code.
fn fallback_code(reason: &str) -> &'static str {
    if reason.contains("byte_limit") {
        "too_large"
    } else if reason.contains("graph_limit") {
        "too_complex"
    } else if reason.contains("parse error") {
        "parse_error"
    } else {
        "text_fallback"
    }
}

/// Highlight spans for one side, per line, sorted, non-overlapping. Where
/// captures nest the innermost wins.
pub(crate) fn syntax_spans(src: &str, parser: &'static TreeSitterConfig) -> Vec<SyntaxSpan> {
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
    collapsed: bool,
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
    // The structural matcher anchors its rows on matched tokens; a line
    // diff's changed blocks are re-paired from whichever end reads alike.
    let rows = match result.file_format {
        FileFormat::SupportedLanguage(_) => aligned_rows(sources, positions),
        _ => line_folds::fallback_rows(sources, positions),
    };
    let (lhs_shown, rhs_shown) = shown_lines(&result.hunks);
    let runs = trim_context(runs(&rows, &lhs_novel, &rhs_novel), &lhs_shown, &rhs_shown);
    let lhs_gaps: Vec<(usize, usize)> = runs
        .iter()
        .filter(|run| run.collapsed)
        .filter_map(|run| match run.sides {
            Pairing::Both { lhs, .. } | Pairing::LeftOnly { lhs } => Some(lhs),
            Pairing::RightOnly { .. } => None,
        })
        .collect();
    let rhs_gaps: Vec<(usize, usize)> = runs
        .iter()
        .filter(|run| run.collapsed)
        .filter_map(|run| match run.sides {
            Pairing::Both { rhs, .. } | Pairing::RightOnly { rhs } => Some(rhs),
            Pairing::LeftOnly { .. } => None,
        })
        .collect();

    let lhs_folds = side_folds(&result.lhs_folds, &lhs_lines, &lhs_gaps);
    let rhs_folds = side_folds(&result.rhs_folds, &rhs_lines, &rhs_gaps);
    let lhs_splits = folds::split_lines(lhs_folds.iter().map(|fold| fold.lines));
    let rhs_splits = folds::split_lines(rhs_folds.iter().map(|fold| fold.lines));
    let (lhs_leaves, rhs_leaves) = split_runs(&runs, &lhs_splits, &rhs_splits);

    let mut ids = Ids::default();
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
    #[cfg(debug_assertions)]
    for side in [&lhs, &rhs] {
        assert_eq!(tree_violation(side), None);
    }
    (lhs, rhs)
}

/// The first way `regions` fails to be a strict tree, if any: a child
/// outside its parent's byte range, siblings out of order or overlapping,
/// or a fold with no children.
#[cfg(any(test, debug_assertions))]
pub(crate) fn tree_violation(regions: &[Region]) -> Option<String> {
    fn pos(position: SourcePos) -> (u32, u32) {
        (position.line, position.column)
    }
    fn walk(regions: &[Region], parent: Option<&Region>) -> Option<String> {
        let mut previous_end = None;
        for region in regions {
            let (start, end) = (pos(region.range.start), pos(region.range.end));
            if let Some(parent) = parent {
                if start < pos(parent.range.start) || end > pos(parent.range.end) {
                    return Some(format!(
                        "region {} outside its parent {}",
                        region.alignment_id, parent.alignment_id
                    ));
                }
            }
            if previous_end.is_some_and(|previous| start < previous) {
                return Some(format!(
                    "region {} overlaps its previous sibling",
                    region.alignment_id
                ));
            }
            previous_end = Some(end);
            if let Node::Fold { children } = &region.node {
                if children.is_empty() {
                    return Some(format!("fold {} has no children", region.alignment_id));
                }
                if let Some(problem) = walk(children, Some(region)) {
                    return Some(problem);
                }
            }
        }
        None
    }
    walk(regions, None)
}

/// One side's folds as nested line spans fitted around the collapsed gaps.
///
/// A fold dropped here is never numbered, so its partner on the other side
/// takes an id of its own (see `Ids::fold`).
fn side_folds<'a>(side: &'a [Fold], lines: &[&str], gaps: &[(usize, usize)]) -> Vec<SideFold<'a>> {
    let spans: Vec<(usize, usize)> = side
        .iter()
        .map(|fold| folds::line_span(fold, lines.len()))
        .collect();
    side.iter()
        .zip(folds::nested_spans(&spans))
        // A fold on a single line hides nothing; it is not a region.
        .filter_map(|(fold, span)| {
            Some(SideFold {
                fold,
                lines: folds::fit_to_gaps(span?, gaps)?,
            })
        })
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
            // A sliver cut off a gap by a fold edge stays open: the same
            // offsets apply to both sides, so paired pieces agree.
            let collapsed = run.collapsed && (cut - at >= MIN_GAP || cut - at == len);
            if let Some((start, _)) = lhs {
                lhs_leaves.push(Leaf {
                    lines: (start + at, start + cut),
                    key,
                    collapsed,
                });
            }
            if let Some((start, _)) = rhs {
                rhs_leaves.push(Leaf {
                    lines: (start + at, start + cut),
                    key,
                    collapsed,
                });
            }
            at = cut;
        }
    }
    (lhs_leaves, rhs_leaves)
}

/// Wire id allocation, in the order regions are built: lhs preorder, then
/// rhs preorder. Ids are dense from 0, and a region paired with one already
/// numbered takes that region's id.
#[derive(Default)]
struct Ids {
    next: u32,
    /// Paired leaves share an id through their `LeafKey`.
    leaves: DftHashMap<LeafKey, u32>,
    /// The id of every fold numbered so far, by the syntax node it was
    /// built on.
    folds: DftHashMap<SyntaxId, u32>,
}

impl Ids {
    fn fresh(&mut self) -> u32 {
        let id = self.next;
        self.next += 1;
        id
    }

    fn leaf(&mut self, key: Option<LeafKey>) -> u32 {
        match key {
            Some(key) => match self.leaves.get(&key) {
                Some(&id) => id,
                None => {
                    let id = self.fresh();
                    self.leaves.insert(key, id);
                    id
                }
            },
            None => self.fresh(),
        }
    }

    /// A fold whose partner is already numbered shares its id. A partner
    /// that is never numbered, because its fold was dropped from its side,
    /// leaves the survivor a fresh id no region on the other side carries.
    fn fold(&mut self, fold: &Fold) -> u32 {
        let shared = fold
            .partner
            .and_then(|partner| self.folds.get(&partner).copied());
        let id = match shared {
            Some(id) => id,
            None => self.fresh(),
        };
        self.folds.insert(fold.syntax_id, id);
        id
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
        id: u32,
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
            alignment_id: open.id,
            fold_state_id: open.id,
            range,
            tags: fold.tags.clone(),
            visibility: Visibility {
                collapsed: false,
                label: fold
                    .summary
                    .clone()
                    .unwrap_or_else(|| fold.placeholder.clone()),
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
                let id = ids.fold(fold.fold);
                stack.push(Open {
                    fold,
                    id,
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
    let (tags, visibility) = if leaf.collapsed {
        (
            vec!["unchanged".to_owned()],
            Visibility {
                collapsed: true,
                label: match end - start {
                    1 => "1 unchanged line".to_owned(),
                    count => format!("{count} unchanged lines"),
                },
            },
        )
    } else {
        (Vec::new(), Visibility::default())
    };
    let id = ids.leaf(leaf.key);
    Region {
        alignment_id: id,
        fold_state_id: id,
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
        tags,
        visibility,
        node: Node::Leaf { changed },
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Params;
    use crate::options::{DiffOptions, DisplayOptions};

    #[test]
    fn a_changed_block_pairs_its_last_lines_when_they_read_alike() {
        // A changed block: an old doc comment and body, then the old
        // signature; the new side has only the new signature. The unchanged
        // row after it is the function body.
        let lhs_lines = [
            "/// Fill in summaries.",
            "fn summarize() {",
            "}",
            "fn qualifies(fold: Fold) -> bool {",
            "    true",
        ];
        let rhs_lines = ["fn qualifies(region: Region) -> bool {", "    true"];
        let rows = vec![
            (Some(0), Some(0)),
            (Some(1), None),
            (Some(2), None),
            (Some(3), None),
            (Some(4), Some(1)),
        ];
        let lhs_novel: BTreeSet<usize> = [0, 1, 2, 3].into_iter().collect();
        let rhs_novel: BTreeSet<usize> = [0].into_iter().collect();
        let realigned = crate::line_folds::align_changed_blocks(
            &rows,
            &lhs_novel,
            &rhs_novel,
            (&lhs_lines, &rhs_lines),
        );
        assert_eq!(
            realigned,
            vec![
                (Some(0), None),
                (Some(1), None),
                (Some(2), None),
                (Some(3), Some(0)),
                (Some(4), Some(1)),
            ]
        );
    }

    #[test]
    fn a_changed_block_keeps_its_rows_when_the_top_reads_as_well() {
        let lhs_lines = ["let a = 1;", "let b = 2;", "done();"];
        let rhs_lines = ["let a = 3;", "done();"];
        let rows = vec![(Some(0), Some(0)), (Some(1), None), (Some(2), Some(1))];
        let lhs_novel: BTreeSet<usize> = [0, 1].into_iter().collect();
        let rhs_novel: BTreeSet<usize> = [0].into_iter().collect();
        assert_eq!(
            crate::line_folds::align_changed_blocks(
                &rows,
                &lhs_novel,
                &rhs_novel,
                (&lhs_lines, &rhs_lines)
            ),
            rows
        );
    }

    #[test]
    fn line_diff_fallbacks_pair_a_changed_signature_with_the_old_one() {
        let before = "fn first() {\n    one();\n}\n\n/// Old doc.\nfn helper() {\n    x();\n    y();\n    z();\n}\n\nfn check(fold: Fold) -> bool {\n    a();\n    b();\n    c();\n}\n";
        let after = "fn first() {\n    one();\n}\n\nfn check(region: Region) -> bool {\n    a();\n    b();\n    c();\n}\n";
        let options = DiffOptions {
            graph_limit: 1,
            ..DiffOptions::default()
        };
        let (_, sides) =
            crate::mutate::summarize::tests::project_with("m.rs", before, after, options);
        let Pairing::Both { lhs, rhs } = &sides else {
            panic!("both sides");
        };
        let signature = |source: &Source, needle: &str| {
            let mut found = None;
            crate::mutate::walk(&source.regions, &mut |region| {
                if matches!(region.node, Node::Leaf { .. })
                    && source
                        .text
                        .lines()
                        .nth(region.range.start.line as usize)
                        .is_some_and(|line| line.starts_with(needle))
                {
                    found = Some(region.alignment_id);
                }
            });
            found.expect("a leaf starts on the signature line")
        };
        assert_eq!(
            signature(lhs, "fn check("),
            signature(rhs, "fn check("),
            "the new signature pairs with the old one, not with the removed doc comment"
        );
    }

    fn refs(lhs: bool, rhs: bool) -> Pairing<FileRef> {
        let file_ref = FileRef {
            path: "a.py".to_owned(),
            oid: String::new(),
            mode: String::new(),
        };
        match (lhs, rhs) {
            (true, true) => Pairing::Both {
                lhs: file_ref.clone(),
                rhs: file_ref,
            },
            (true, false) => Pairing::LeftOnly { lhs: file_ref },
            (false, true) => Pairing::RightOnly { rhs: file_ref },
            (false, false) => panic!("a file has a side"),
        }
    }

    fn project(path: &str, lhs: &str, rhs: &str, context: usize) -> Diff {
        project_with(path, lhs, rhs, context, DiffOptions::default())
    }

    /// `graph_limit: 1` forces the text-diff fallback for any real change.
    fn project_with(
        path: &str,
        lhs: &str,
        rhs: &str,
        context: usize,
        options: DiffOptions,
    ) -> Diff {
        let result = DiffResult::from_sources_with_options(
            path,
            lhs,
            rhs,
            &Params::default(),
            &DisplayOptions {
                num_context_lines: context as u32,
                ..DisplayOptions::default()
            },
            &options,
        );
        diff(
            &result,
            Inputs {
                file: &refs(!lhs.is_empty(), !rhs.is_empty()),
                sizes: (lhs.len() as u64, rhs.len() as u64),
                syntax: (Vec::new(), Vec::new()),
            },
        )
    }

    fn sources(diff: &Diff) -> (Option<&Source>, Option<&Source>) {
        match diff {
            Diff::Text { sides, .. } => match sides {
                Pairing::Both { lhs, rhs } => (Some(lhs), Some(rhs)),
                Pairing::LeftOnly { lhs } => (Some(lhs), None),
                Pairing::RightOnly { rhs } => (None, Some(rhs)),
            },
            Diff::Binary { .. } => panic!("text diff"),
        }
    }

    fn leaves(regions: &[Region]) -> Vec<&Region> {
        let mut out = Vec::new();
        for region in regions {
            match &region.node {
                Node::Leaf { .. } => out.push(region),
                Node::Fold { children } => out.extend(leaves(children)),
            }
        }
        out
    }

    fn all(regions: &[Region]) -> Vec<&Region> {
        let mut out = Vec::new();
        for region in regions {
            out.push(region);
            if let Node::Fold { children } = &region.node {
                out.extend(all(children));
            }
        }
        out
    }

    fn line_count(text: &str) -> u32 {
        text.split_terminator('\n').count() as u32
    }

    fn assert_tiles(source: &Source) {
        let mut at = 0;
        for leaf in leaves(&source.regions) {
            assert_eq!(leaf.range.start.line, at, "gap before {leaf:?}");
            assert_eq!(leaf.range.start.column, 0);
            assert_eq!(leaf.range.end.column, 0);
            assert!(leaf.range.end.line > at, "empty leaf {leaf:?}");
            at = leaf.range.end.line;
        }
        assert_eq!(at, line_count(&source.text));
    }

    fn assert_folds_hold_children(regions: &[Region]) {
        assert_eq!(tree_violation(regions), None);
        for region in regions {
            if let Node::Fold { children } = &region.node {
                assert!(!children.is_empty(), "fold without children {region:?}");
                let (start, end) = region.range.lines_spanned();
                let mut at = start;
                for child in children {
                    let (child_start, child_end) = child.range.lines_spanned();
                    assert_eq!(child_start, at, "hole inside {region:?}");
                    at = child_end;
                }
                assert_eq!(at, end, "fold {region:?} not tiled by its children");
                assert_folds_hold_children(children);
            }
        }
    }

    impl SourceRange {
        fn lines_spanned(&self) -> (u32, u32) {
            let range = self.lines();
            (range.start, range.end)
        }
    }

    const RUST_LHS: &str = "fn f(a: u32) -> u32 {\n    let x = a + 1;\n    let y = x * 2;\n    x + y\n}\n\nfn keep() -> u32 {\n    let k = 1;\n    let m = 2;\n    k + m\n}\n";
    const RUST_RHS: &str = "fn f(a: u32, b: u32) -> u32 {\n    let x = a + b;\n    let y = x * 2;\n    x + y\n}\n\nfn keep() -> u32 {\n    let k = 1;\n    let m = 2;\n    k + m\n}\n\nfn added() -> u32 {\n    let p = 3;\n    let q = 4;\n    p + q\n}\n";

    fn fold_ids(source: &Source) -> BTreeMap<u32, u32> {
        all(&source.regions)
            .into_iter()
            .filter(|r| matches!(r.node, Node::Fold { .. }))
            .map(|r| (r.range.start.line, r.alignment_id))
            .collect()
    }

    #[test]
    fn a_fold_ending_inside_a_gap_does_not_split_it() {
        // The inner block's closer sits inside a long unchanged stretch that
        // continues in the enclosing function: one gap, not two back to back.
        let body = (1..=7)
            .map(|n| format!("        u{n}();\n"))
            .collect::<Vec<_>>()
            .concat();
        let tail = (1..=5)
            .map(|n| format!("    v{n}();\n"))
            .collect::<Vec<_>>()
            .concat();
        let lhs = format!("fn f() {{\n    if a {{\n        x();\n{body}    }}\n{tail}}}\n");
        let rhs = format!("fn f() {{\n    if a {{\n        y();\n{body}    }}\n{tail}}}\n");
        let diff = project("a.rs", &lhs, &rhs, 1);
        let (lhs_src, rhs_src) = sources(&diff);
        for source in [lhs_src.unwrap(), rhs_src.unwrap()] {
            assert_tiles(source);
            let gaps: Vec<&Region> = all(&source.regions)
                .into_iter()
                .filter(|r| {
                    matches!(r.node, Node::Leaf { .. })
                        && r.visibility.collapsed
                        && r.tags.iter().any(|t| t == "unchanged")
                })
                .collect();
            assert_eq!(gaps.len(), 1, "one gap: {gaps:?}");
            assert!(gaps[0].range.end.line - gaps[0].range.start.line >= 10);
        }
    }

    #[test]
    fn a_closer_and_an_opener_on_one_line_yield_a_strict_tree() {
        let lhs = "fn f() {\n    for x in [\n        1,\n        2,\n    ] {\n        use_it(x);\n        more(x);\n    }\n}\n";
        let rhs = "fn f() {\n    for x in [\n        1,\n        2,\n        3,\n    ] {\n        use_it(x);\n        more(x);\n    }\n}\n";
        let diff = project("a.rs", lhs, rhs, 3);
        let (lhs, rhs) = sources(&diff);
        for source in [lhs.unwrap(), rhs.unwrap()] {
            assert_tiles(source);
            assert_folds_hold_children(&source.regions);
            let folds: Vec<&Region> = all(&source.regions)
                .into_iter()
                .filter(|region| matches!(region.node, Node::Fold { .. }))
                .collect();
            let collection = folds
                .iter()
                .find(|fold| fold.tags.iter().any(|tag| tag == "collection"))
                .expect("the array is a fold");
            let body = folds
                .iter()
                .find(|fold| fold.tags.iter().any(|tag| tag == "body") && fold.range.start.line > 0)
                .expect("the loop body is a fold");
            assert_eq!(
                collection.range.end.line, body.range.start.line,
                "the collection gives its closing line to the body that opens there"
            );
            assert_eq!(body.range.start.column, 0);
            assert_eq!(collection.range.end.column, 0);
        }
    }

    #[test]
    fn a_parse_error_fallback_numbers_its_folds() {
        // Both sides hold a stray `)`, so the parse-error limit of zero sends
        // the file to a line diff; the folds still come from the parse.
        let lhs = format!("{RUST_LHS})\n");
        let rhs = format!("{RUST_RHS})\n");
        let diff = project_with(
            "a.rs",
            &lhs,
            &rhs,
            3,
            DiffOptions {
                parse_error_limit: 0,
                ..DiffOptions::default()
            },
        );
        let Diff::Text { stats, .. } = &diff else {
            panic!("text diff");
        };
        assert_eq!(stats.fallback.as_ref().unwrap().code, "parse_error");
        let (lhs, rhs) = sources(&diff);
        let (lhs_folds, rhs_folds) = (fold_ids(lhs.unwrap()), fold_ids(rhs.unwrap()));
        let lhs_ids: BTreeSet<u32> = lhs_folds.values().copied().collect();
        let rhs_ids: BTreeSet<u32> = rhs_folds.values().copied().collect();
        assert_eq!(
            lhs_ids.len(),
            lhs_folds.len(),
            "lhs folds have distinct ids"
        );
        assert_eq!(
            rhs_ids.len(),
            rhs_folds.len(),
            "rhs folds have distinct ids"
        );
        assert_eq!(
            lhs_folds[&0], rhs_folds[&0],
            "f pairs across a changed header"
        );
        assert!(!lhs_ids.contains(&rhs_folds[&12]), "added is rhs-only");
    }

    #[test]
    fn folds_pair_on_either_engine() {
        for options in [
            DiffOptions::default(),
            DiffOptions {
                graph_limit: 1,
                ..DiffOptions::default()
            },
        ] {
            let structural = options.graph_limit != 1;
            let diff = project_with("a.rs", RUST_LHS, RUST_RHS, 3, options);
            let Diff::Text { stats, .. } = &diff else {
                panic!("text diff");
            };
            assert_eq!(stats.fallback.is_none(), structural);
            let (lhs, rhs) = sources(&diff);
            let (lhs, rhs) = (lhs.unwrap(), rhs.unwrap());
            assert_tiles(lhs);
            assert_tiles(rhs);
            let (lhs_folds, rhs_folds) = (fold_ids(lhs), fold_ids(rhs));
            // `f` changed its signature and stays paired: through the matcher
            // on the structural path, through its aligned header line on the
            // fallback. `keep` is untouched. `added` is rhs-only.
            assert_eq!(
                lhs_folds[&0], rhs_folds[&0],
                "f pairs across a changed header"
            );
            assert_eq!(lhs_folds[&6], rhs_folds[&6], "keep pairs");
            assert!(
                !lhs_folds.values().any(|id| *id == rhs_folds[&12]),
                "added is rhs-only"
            );
            assert_eq!(lhs_folds.len(), 2);
            assert_eq!(rhs_folds.len(), 3);
        }
    }

    #[test]
    fn structural_folds_pair_exactly_as_the_matcher_recorded() {
        let result = DiffResult::from_sources_with_options(
            "a.rs",
            RUST_LHS,
            RUST_RHS,
            &Params::default(),
            &DisplayOptions::default(),
            &DiffOptions::default(),
        );
        let diff = project("a.rs", RUST_LHS, RUST_RHS, 3);
        let (lhs, rhs) = sources(&diff);
        let (lhs_folds, rhs_folds) = (fold_ids(lhs.unwrap()), fold_ids(rhs.unwrap()));
        let rhs_ids: BTreeSet<u32> = rhs_folds.values().copied().collect();
        for fold in &result.lhs_folds {
            let id = lhs_folds[&(fold.range.start.line.as_usize() as u32)];
            let matcher_paired = fold.counterpart(&result.rhs_folds).is_some();
            assert_eq!(rhs_ids.contains(&id), matcher_paired, "{:?}", fold.range);
        }
    }

    #[test]
    fn a_fold_dropped_on_one_side_leaves_its_partner_unshared() {
        // The matcher pairs the two arrays, but the lhs array sits on one
        // line: it hides nothing and is not a region.
        let lhs = "fn f() {\n    let v = [1, 2];\n    work(v);\n}\n";
        let rhs = "fn f() {\n    let v = [\n        1,\n        2,\n    ];\n    work(v);\n}\n";
        let result = DiffResult::from_sources("a.rs", lhs, rhs);
        let collection = |folds: &[Fold]| -> (SyntaxId, Option<SyntaxId>) {
            let fold = folds
                .iter()
                .find(|fold| fold.tags.iter().any(|tag| tag == "collection"))
                .expect("the array is a fold");
            (fold.syntax_id, fold.partner)
        };
        let (lhs_array, lhs_partner) = collection(&result.lhs_folds);
        let (rhs_array, rhs_partner) = collection(&result.rhs_folds);
        assert_eq!(
            (lhs_partner, rhs_partner),
            (Some(rhs_array), Some(lhs_array)),
            "the matcher pairs the arrays"
        );
        let diff = project("a.rs", lhs, rhs, 3);
        let (lhs, rhs) = sources(&diff);
        let (lhs, rhs) = (lhs.unwrap(), rhs.unwrap());
        let lhs_ids: BTreeSet<u32> = all(&lhs.regions).iter().map(|r| r.alignment_id).collect();
        let array = all(&rhs.regions)
            .into_iter()
            .find(|r| r.tags.iter().any(|tag| tag == "collection"))
            .expect("the rhs array is a region");
        assert!(
            !lhs_ids.contains(&array.alignment_id),
            "no lhs region claims the dropped fold's id"
        );
    }

    #[test]
    fn a_line_diff_pairs_a_reflowed_signature_through_its_body() {
        // The signature moves onto three lines, so the header rows no
        // longer align, but the body is untouched: still the same function.
        let lhs =
            "fn f(a: u32, b: u32) -> u32 {\n    let x = a + b;\n    let y = x * 2;\n    x + y\n}\n";
        let rhs = "fn f(\n    a: u32,\n    b: u32,\n) -> u32 {\n    let x = a + b;\n    let y = x * 2;\n    x + y\n}\n";
        let diff = project_with(
            "a.rs",
            lhs,
            rhs,
            3,
            DiffOptions {
                graph_limit: 1,
                ..DiffOptions::default()
            },
        );
        let (lhs_src, rhs_src) = sources(&diff);
        let (lhs_folds, rhs_folds) = (fold_ids(lhs_src.unwrap()), fold_ids(rhs_src.unwrap()));
        assert_eq!(lhs_folds.len(), 1);
        assert_eq!(rhs_folds.len(), 1);
        assert_eq!(
            lhs_folds.values().next(),
            rhs_folds.values().next(),
            "the reflowed function keeps one alignment id"
        );
    }

    #[test]
    fn line_diff_content_pairing_picks_the_candidate_with_the_most_paired_lines() {
        // Both rhs functions have reflowed signatures and both hold a line
        // `f` also holds (`let x = ...`), but only `g` holds `f`'s long
        // body. Order is preserved so nothing crosses.
        let lhs = "fn h(b: u32) -> u32 {\n    let x = b;\n    b\n}\n\nfn f(a: u32) -> u32 {\n    let x = a;\n    let y = x * 2;\n    let z = y * 3;\n    x + y + z\n}\n";
        let rhs = "fn h(\n    b: u32,\n) -> u32 {\n    let x = b;\n    b\n}\n\nfn g(\n    a: u32,\n) -> u32 {\n    let x = a;\n    let y = x * 2;\n    let z = y * 3;\n    x + y + z\n}\n";
        let diff = project_with(
            "a.rs",
            lhs,
            rhs,
            3,
            DiffOptions {
                graph_limit: 1,
                ..DiffOptions::default()
            },
        );
        let (lhs_src, rhs_src) = sources(&diff);
        let (lhs_folds, rhs_folds) = (fold_ids(lhs_src.unwrap()), fold_ids(rhs_src.unwrap()));
        // Fold headers sit on the `{` line: lhs h=0, f=5; rhs h=2, g=9.
        assert_eq!(lhs_folds[&0], rhs_folds[&2], "h pairs with h");
        assert_eq!(lhs_folds[&5], rhs_folds[&9], "f pairs with g by its body");
    }

    #[test]
    fn line_diff_content_pairing_never_crosses_an_existing_pair() {
        // `keep` pairs by header on both sides. `f` moved below `keep` on
        // the rhs with a reflowed signature; pairing it would cross `keep`,
        // so it stays one-sided on each side.
        let lhs = "fn f(a: u32) -> u32 {\n    let x = a + 1;\n    let y = x * 2;\n    x + y\n}\n\nfn keep() -> u32 {\n    let k = 1;\n    let m = 2;\n    k + m\n}\n";
        let rhs = "fn keep() -> u32 {\n    let k = 1;\n    let m = 2;\n    k + m\n}\n\nfn f(\n    a: u32,\n) -> u32 {\n    let x = a + 1;\n    let y = x * 2;\n    x + y\n}\n";
        let diff = project_with(
            "a.rs",
            lhs,
            rhs,
            3,
            DiffOptions {
                graph_limit: 1,
                ..DiffOptions::default()
            },
        );
        let (lhs_src, rhs_src) = sources(&diff);
        let (lhs_folds, rhs_folds) = (fold_ids(lhs_src.unwrap()), fold_ids(rhs_src.unwrap()));
        assert_eq!(lhs_folds[&6], rhs_folds[&0], "keep pairs by header");
        assert_ne!(
            lhs_folds[&0], rhs_folds[&8],
            "f would cross keep, so it stays unpaired"
        );
    }

    #[test]
    fn the_fallback_keeps_folds_and_enclosing_context() {
        let diff = project_with(
            "a.rs",
            RUST_LHS,
            RUST_RHS,
            1,
            DiffOptions {
                graph_limit: 1,
                ..DiffOptions::default()
            },
        );
        let Diff::Text { stats, .. } = &diff else {
            panic!("text diff");
        };
        assert_eq!(stats.fallback.as_ref().unwrap().code, "too_complex");
        let (_, rhs) = sources(&diff);
        let rhs = rhs.unwrap();
        let bodies: Vec<_> = all(&rhs.regions)
            .into_iter()
            .filter(|r| r.tags.iter().any(|tag| tag == "body"))
            .map(|r| r.range.start.line)
            .collect();
        // `keep` is untouched and lies inside a collapsed gap, so it is not a
        // region; the changed `f` and the new `added` are folds.
        assert_eq!(bodies, vec![0, 12]);
        // The change inside `f` keeps its header line open, not in a gap.
        let gaps: Vec<_> = leaves(&rhs.regions)
            .into_iter()
            .filter(|r| r.visibility.collapsed)
            .map(|r| r.range.lines_spanned())
            .collect();
        assert!(gaps.iter().all(|(start, _)| *start != 0), "{gaps:?}");
    }

    #[test]
    fn leaves_tile_both_sides_and_paired_leaves_share_ids() {
        let lhs = "import os\n\ndef f():\n    x = 1\n    return x\n";
        let rhs =
            "import os\n\ndef f():\n    x = 2\n    return x\n\ndef g():\n    y = 3\n    return y\n";
        let diff = project("a.py", lhs, rhs, 3);
        let (lhs, rhs) = sources(&diff);
        let (lhs, rhs) = (lhs.unwrap(), rhs.unwrap());
        assert_tiles(lhs);
        assert_tiles(rhs);
        assert_folds_hold_children(&lhs.regions);
        assert_folds_hold_children(&rhs.regions);
        let lhs_ids: BTreeSet<u32> = all(&lhs.regions).iter().map(|r| r.alignment_id).collect();
        let rhs_ids: BTreeSet<u32> = all(&rhs.regions).iter().map(|r| r.alignment_id).collect();
        // Ids are dense and assigned lhs first.
        let max = lhs_ids.iter().chain(&rhs_ids).max().copied().unwrap();
        assert_eq!(
            lhs_ids.union(&rhs_ids).copied().collect::<Vec<_>>(),
            (0..=max).collect::<Vec<_>>()
        );
        for (lhs_leaf, rhs_leaf) in leaves(&lhs.regions).iter().zip(leaves(&rhs.regions)) {
            if lhs_leaf.alignment_id == rhs_leaf.alignment_id {
                assert_eq!(
                    lhs_leaf.range.lines_spanned().1 - lhs_leaf.range.lines_spanned().0,
                    rhs_leaf.range.lines_spanned().1 - rhs_leaf.range.lines_spanned().0,
                    "paired leaves have equal length"
                );
            }
        }
        // The new function exists on the rhs only.
        let rhs_only: Vec<_> = rhs_ids.difference(&lhs_ids).collect();
        assert!(!rhs_only.is_empty());
        // Python's body fold is the block, which starts on the line after `def`.
        let new_fold = all(&rhs.regions)
            .into_iter()
            .find(|r| matches!(r.node, Node::Fold { .. }) && r.range.start.line == 7)
            .expect("the added function is a fold");
        assert!(rhs_only.contains(&&new_fold.alignment_id));
        assert_eq!(new_fold.tags, vec!["body", "function"]);
        assert_eq!(new_fold.visibility.label, "Body");
    }

    #[test]
    fn the_changed_body_is_a_paired_fold_with_a_novel_leaf_inside() {
        let lhs = "def f():\n    a = 1\n    b = 2\n    return a\n";
        let rhs = "def f():\n    a = 1\n    b = 3\n    return a\n";
        let diff = project("a.py", lhs, rhs, 3);
        let (lhs, rhs) = sources(&diff);
        let (lhs, rhs) = (lhs.unwrap(), rhs.unwrap());
        let fold = |source: &Source| {
            let folds: Vec<_> = all(&source.regions)
                .into_iter()
                .filter(|r| matches!(r.node, Node::Fold { .. }))
                .collect();
            assert_eq!(folds.len(), 1);
            folds[0].clone()
        };
        let lhs_fold = fold(lhs);
        let rhs_fold = fold(rhs);
        assert_eq!(lhs_fold.alignment_id, rhs_fold.alignment_id);
        assert_eq!(lhs_fold.range.lines_spanned(), (1, 4));
        let novel: Vec<_> = leaves(&rhs.regions)
            .into_iter()
            .filter(|leaf| matches!(&leaf.node, Node::Leaf { changed } if !changed.is_empty()))
            .collect();
        assert_eq!(novel.len(), 1);
        assert_eq!(novel[0].range.lines_spanned(), (2, 3));
        let Node::Leaf { changed } = &novel[0].node else {
            unreachable!()
        };
        // Only the changed token is painted, not the whole line.
        assert_eq!(
            changed,
            &[Span {
                line: 2,
                start_column: 8,
                end_column: 9
            }]
        );
    }

    #[test]
    fn a_fully_new_line_is_painted_whole_and_blank_lines_not_at_all() {
        let diff = project("a.py", "x = 1\n", "x = 1\n\ny = 2\n", 3);
        let (_, rhs) = sources(&diff);
        let changed: Vec<Span> = leaves(&rhs.unwrap().regions)
            .into_iter()
            .filter_map(|leaf| match &leaf.node {
                Node::Leaf { changed } => Some(changed.clone()),
                Node::Fold { .. } => None,
            })
            .flatten()
            .collect();
        assert_eq!(
            changed,
            vec![Span {
                line: 2,
                start_column: 0,
                end_column: 5
            }]
        );
    }

    #[test]
    fn long_unchanged_runs_collapse_to_the_context_width() {
        let body = (0..20)
            .map(|i| format!("x{i} = {i}\n"))
            .collect::<Vec<_>>()
            .concat();
        let lhs = format!("{body}changed = 1\n{body}");
        let rhs = format!("{body}changed = 2\n{body}");
        let diff = project("a.py", &lhs, &rhs, 3);
        let (lhs, _) = sources(&diff);
        let leaves = leaves(&lhs.unwrap().regions);
        let spans: Vec<_> = leaves
            .iter()
            .map(|leaf| {
                (
                    leaf.range.lines_spanned(),
                    leaf.visibility.collapsed,
                    leaf.tags.clone(),
                )
            })
            .collect();
        assert_eq!(
            spans,
            vec![
                ((0, 17), true, vec!["unchanged".to_owned()]),
                ((17, 20), false, vec![]),
                ((20, 21), false, vec![]),
                ((21, 24), false, vec![]),
                ((24, 41), true, vec!["unchanged".to_owned()]),
            ]
        );
        assert_eq!(leaves[0].visibility.label, "17 unchanged lines");
    }

    #[test]
    fn folds_inside_a_gap_are_dropped_and_the_gap_stays_whole() {
        // Three unchanged functions sit between two changes. They would be
        // hidden inside the gap, so they are not regions and do not split it.
        let unchanged = (0..3)
            .map(|i| format!("def f{i}():\n    a = {i}\n    b = {i}\n    return a + b\n\n"))
            .collect::<Vec<_>>()
            .concat();
        let lhs = format!("first = 1\n\n{unchanged}last = 1\n");
        let rhs = format!("first = 2\n\n{unchanged}last = 2\n");
        let diff = project("a.py", &lhs, &rhs, 1);
        let (lhs, rhs) = sources(&diff);
        let (lhs, rhs) = (lhs.unwrap(), rhs.unwrap());
        assert!(all(&lhs.regions)
            .iter()
            .all(|r| matches!(r.node, Node::Leaf { .. })));
        let gaps: Vec<_> = leaves(&rhs.regions)
            .into_iter()
            .filter(|leaf| leaf.visibility.collapsed)
            .map(|leaf| leaf.range.lines_spanned())
            .collect();
        assert_eq!(gaps, vec![(2, 16)]);
        assert_tiles(lhs);
        assert_tiles(rhs);
    }

    #[test]
    fn slivers_cut_from_a_gap_by_a_fold_edge_stay_open() {
        // The changed function's fold edge lands one line into the gap
        // before it; that one line is not worth a fold row.
        let head = (0..8)
            .map(|i| format!("x{i} = {i}\n"))
            .collect::<Vec<_>>()
            .concat();
        let lhs = format!("{head}\ndef f():\n    a = 1\n    b = 1\n    c = 1\n    return 1\n");
        let rhs = format!("{head}\ndef f():\n    a = 1\n    b = 1\n    c = 1\n    return 2\n");
        let diff = project("a.py", &lhs, &rhs, 1);
        let (_, rhs) = sources(&diff);
        let rhs = rhs.unwrap();
        for leaf in leaves(&rhs.regions) {
            let (start, end) = leaf.range.lines_spanned();
            assert!(
                !leaf.visibility.collapsed || end - start >= MIN_GAP as u32,
                "collapsed sliver {start}..{end}"
            );
        }
        assert_tiles(rhs);
    }

    #[test]
    fn the_enclosing_header_stays_open_above_a_deep_change() {
        // A change ten lines into a function: the `def` line is syntax
        // context, so it is shown even though it is far outside the padding.
        let body = (0..10)
            .map(|i| format!("    a{i} = {i}\n"))
            .collect::<Vec<_>>()
            .concat();
        let lhs = format!("def outer():\n{body}    return 1\n");
        let rhs = format!("def outer():\n{body}    return 2\n");
        let diff = project("a.py", &lhs, &rhs, 1);
        let (_, rhs) = sources(&diff);
        let rhs = rhs.unwrap();
        let open_lines: Vec<u32> = leaves(&rhs.regions)
            .into_iter()
            .filter(|leaf| !leaf.visibility.collapsed)
            .flat_map(|leaf| {
                let (start, end) = leaf.range.lines_spanned();
                start..end
            })
            .collect();
        assert!(
            open_lines.contains(&0),
            "header line hidden: {open_lines:?}"
        );
        assert!(
            !open_lines.contains(&5),
            "middle of the body should be a gap"
        );
        assert_tiles(rhs);
    }

    #[test]
    fn a_fold_edge_splits_paired_leaves_on_both_sides() {
        // The unchanged function inside the gap forces a split on both sides
        // at the same offset, even though only the rhs shifted.
        let lhs = "a = 1\n\ndef f():\n    return 1\n\nz = 1\n";
        let rhs = "a = 2\n\ndef f():\n    return 1\n\nz = 1\n";
        let diff = project("a.py", lhs, rhs, 1);
        let (lhs, rhs) = sources(&diff);
        let (lhs, rhs) = (lhs.unwrap(), rhs.unwrap());
        assert_tiles(lhs);
        assert_tiles(rhs);
        assert_folds_hold_children(&lhs.regions);
        assert_folds_hold_children(&rhs.regions);
        let lhs_leaves: Vec<_> = leaves(&lhs.regions)
            .iter()
            .map(|l| (l.alignment_id, l.range.lines_spanned()))
            .collect();
        let rhs_leaves: Vec<_> = leaves(&rhs.regions)
            .iter()
            .map(|l| (l.alignment_id, l.range.lines_spanned()))
            .collect();
        assert_eq!(lhs_leaves, rhs_leaves);
    }

    #[test]
    fn stats_count_text_and_syntax_lines_separately() {
        let diff = project("a.py", "x = 1\n", "x = 1 # same\n", 3);
        let Diff::Text { stats, .. } = &diff else {
            panic!("text")
        };
        assert_eq!(
            stats.textual,
            LineCounts {
                added: 1,
                removed: 1
            }
        );
        assert!(stats.fallback.is_none());
        let diff = project("a.unknownext", "x\n", "y\n", 3);
        let Diff::Text { stats, .. } = &diff else {
            panic!("text")
        };
        assert_eq!(
            stats.fallback.as_ref().unwrap().code,
            "unsupported_language"
        );
    }

    #[test]
    fn one_sided_files_have_one_source_and_no_shared_ids() {
        let diff = project("a.py", "", "def f():\n    return 1\n", 3);
        let (lhs, rhs) = sources(&diff);
        assert!(lhs.is_none());
        let rhs = rhs.unwrap();
        assert_tiles(rhs);
        assert!(leaves(&rhs.regions)
            .iter()
            .all(|leaf| !leaf.visibility.collapsed));
    }

    #[test]
    fn identical_files_are_one_collapsed_leaf() {
        let diff = project("a.py", "x = 1\ny = 2\n", "x = 1\ny = 2\n", 3);
        let (lhs, rhs) = sources(&diff);
        let lhs = lhs.unwrap();
        assert_eq!(leaves(&lhs.regions).len(), 1);
        assert!(leaves(&lhs.regions)[0].visibility.collapsed);
        assert_eq!(
            lhs.regions[0].alignment_id,
            rhs.unwrap().regions[0].alignment_id
        );
    }

    #[test]
    fn binary_sides_carry_sizes() {
        let result = DiffResult {
            display_path: "a.bin".to_owned(),
            extra_info: None,
            file_format: FileFormat::Binary,
            lhs_src: FileContent::Binary,
            rhs_src: FileContent::Binary,
            hunks: vec![],
            lhs_folds: vec![],
            rhs_folds: vec![],
            lhs_positions: vec![],
            rhs_positions: vec![],
            has_byte_changes: Some((3, 5)),
            has_syntactic_changes: false,
        };
        let diff = diff(
            &result,
            Inputs {
                file: &refs(true, true),
                sizes: (3, 5),
                syntax: (Vec::new(), Vec::new()),
            },
        );
        let Diff::Binary {
            sides: Pairing::Both { lhs, rhs },
        } = diff
        else {
            panic!("a binary diff with both sides: {diff:?}");
        };
        assert_eq!((lhs.size, rhs.size), (3, 5));
    }

    #[test]
    fn syntax_spans_are_per_line_sorted_and_innermost() {
        let parser = crate::parse::tree_sitter_parser::from_language(
            crate::parse::guess_language::Language::Python,
        );
        let spans = syntax_spans("def f(x):\n    return \"a\"\n", parser);
        for pair in spans.windows(2) {
            assert!(
                pair[0].line < pair[1].line
                    || (pair[0].line == pair[1].line && pair[0].end_column <= pair[1].start_column),
                "{pair:?}"
            );
        }
        assert!(spans
            .iter()
            .any(|span| span.capture == "keyword" && span.line == 0));
        assert!(spans
            .iter()
            .any(|span| span.capture.starts_with("string") && span.line == 1));
    }
}
