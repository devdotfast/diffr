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
//! other so paired leaves stay equal in length. Nothing starts collapsed.
use super::{
    BinaryRef, Diff, FileRef, LineCounts, Node, Problem, Region, Source, SourcePos, SourceRange,
    Span, Stats, Visibility, ROOT,
};
use crate::display::line_layout::{aligned_rows, novel_lines, runs, Run};
use crate::hash::DftHashMap;
use crate::line_parser;
use crate::pairing::Pairing;
use crate::parse::folds::{self, Fold, FoldMatch};
use crate::parse::syntax::{MatchKind, MatchedPos, SyntaxId};
use crate::summary::{DiffResult, FallbackCause, FileContent, FileFormat};
use std::collections::{BTreeMap, BTreeSet};

/// Everything the projection needs besides the diff itself.
pub(crate) struct Inputs<'a> {
    /// Which sides the file exists on; a one-sided file gets one source.
    pub(crate) file: &'a Pairing<FileRef>,
    /// Byte length of each side's content, for binary files.
    pub(crate) sizes: (u64, u64),
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
    let sides = pair(
        inputs.file,
        Source {
            text: lhs_src.to_owned(),
            regions: lhs_regions,
        },
        Source {
            text: rhs_src.to_owned(),
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
        FileFormat::TextFallback { cause, reason } => Some(Problem {
            code: fallback_code(*cause).to_owned(),
            message: reason.clone(),
        }),
        FileFormat::Binary => unreachable!("binary files never reach text stats"),
    };
    Stats { textual, fallback }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Params;
    use crate::options::{DiffOptions, DisplayOptions};

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

    fn project(path: &str, lhs: &str, rhs: &str) -> Diff {
        project_with(path, lhs, rhs, DiffOptions::default())
    }

    /// `graph_limit: 1` forces the text-diff fallback for any real change.
    fn project_with(path: &str, lhs: &str, rhs: &str, options: DiffOptions) -> Diff {
        let result = DiffResult::from_sources_with_options(
            path,
            lhs,
            rhs,
            &Params::default(),
            &DisplayOptions::default(),
            &options,
        )
        .unwrap();
        diff(
            &result,
            Inputs {
                file: &refs(!lhs.is_empty(), !rhs.is_empty()),
                sizes: (lhs.len() as u64, rhs.len() as u64),
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

    fn alignment(leaf: &Region) -> u32 {
        match leaf.node {
            Node::Leaf { alignment_id, .. } => alignment_id,
            Node::Fold { .. } => panic!("a fold has no alignment_id: {leaf:?}"),
        }
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
        /// The lines this range touches, half-open. A range ending at
        /// column zero does not touch its end line.
        fn lines_spanned(&self) -> (u32, u32) {
            let end = if self.end.column == 0 {
                self.end.line
            } else {
                self.end.line + 1
            };
            (self.start.line, end)
        }
    }

    const RUST_LHS: &str = "fn f(a: u32) -> u32 {\n    let x = a + 1;\n    let y = x * 2;\n    x + y\n}\n\nfn keep() -> u32 {\n    let k = 1;\n    let m = 2;\n    k + m\n}\n";
    const RUST_RHS: &str = "fn f(a: u32, b: u32) -> u32 {\n    let x = a + b;\n    let y = x * 2;\n    x + y\n}\n\nfn keep() -> u32 {\n    let k = 1;\n    let m = 2;\n    k + m\n}\n\nfn added() -> u32 {\n    let p = 3;\n    let q = 4;\n    p + q\n}\n";

    fn fold_ids(source: &Source) -> BTreeMap<u32, u32> {
        all(&source.regions)
            .into_iter()
            .filter(|r| matches!(r.node, Node::Fold { .. }))
            .map(|r| (r.range.start.line, r.id))
            .collect()
    }

    fn fold_states(source: &Source) -> BTreeMap<u32, u32> {
        all(&source.regions)
            .into_iter()
            .filter(|r| matches!(r.node, Node::Fold { .. }))
            .map(|r| (r.range.start.line, r.fold_state_id))
            .collect()
    }

    #[test]
    fn a_closer_and_an_opener_on_one_line_yield_a_strict_tree() {
        let lhs = "fn f() {\n    for x in [\n        1,\n        2,\n    ] {\n        use_it(x);\n        more(x);\n    }\n}\n";
        let rhs = "fn f() {\n    for x in [\n        1,\n        2,\n        3,\n    ] {\n        use_it(x);\n        more(x);\n    }\n}\n";
        let diff = project("a.rs", lhs, rhs);
        let (lhs, rhs) = sources(&diff);
        for source in [lhs.unwrap(), rhs.unwrap()] {
            assert_tiles(source);
            assert_folds_hold_children(&source.regions);
            let folds: Vec<&Region> = all(&source.regions)
                .into_iter()
                .filter(|region| matches!(region.node, Node::Fold { .. }))
                .collect();
            // The array's elements open on line 2; `] {` closes it and
            // opens the loop, and belongs to neither fold.
            let collection = folds
                .iter()
                .find(|fold| fold.range.start.line == 2)
                .expect("the array is a fold");
            let body = folds
                .iter()
                .find(|fold| fold.range.start.line > collection.range.end.line)
                .expect("the loop body is a fold");
            assert!(
                collection.range.end.line < body.range.start.line,
                "the line that closes the collection and opens the body is in neither"
            );
            assert_eq!(body.range.start.column, 0);
            assert_eq!(collection.range.end.column, 0);
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
        )
        .unwrap();
        let diff = project("a.rs", RUST_LHS, RUST_RHS);
        let (lhs, rhs) = sources(&diff);
        let lhs_folds = fold_ids(lhs.unwrap());
        let (lhs_states, rhs_states) = (fold_states(lhs.unwrap()), fold_states(rhs.unwrap()));
        let rhs_ids: BTreeSet<u32> = all(&rhs.unwrap().regions).iter().map(|r| r.id).collect();
        let rhs_states: BTreeSet<u32> = rhs_states.values().copied().collect();
        let lhs_lines: Vec<&str> = RUST_LHS.split_terminator('\n').collect();
        for fold in &result.lhs_folds {
            // A fold covers its body: its region starts after the line the
            // `{` opens on.
            let line = folds::line_span(fold, &lhs_lines).0 as u32;
            let (id, state) = (lhs_folds[&line], lhs_states[&line]);
            let matcher_paired = matches!(fold.match_kind, FoldMatch::Matched { .. });
            assert_eq!(
                rhs_states.contains(&state),
                matcher_paired,
                "{:?}",
                fold.range
            );
            // A region's `id` names it alone.
            assert!(!rhs_ids.contains(&id), "{:?}", fold.range);
        }
    }

    #[test]
    fn a_fold_dropped_on_one_side_leaves_its_partner_unshared() {
        // The matcher pairs the two arrays, but the lhs array sits on one
        // line: it hides nothing and is not a region.
        let lhs = "fn f() {\n    let v = [1, 2];\n    work(v);\n}\n";
        let rhs = "fn f() {\n    let v = [\n        1,\n        2,\n    ];\n    work(v);\n}\n";
        let result = DiffResult::from_sources("a.rs", lhs, rhs);
        let collection = |folds: &[Fold]| -> (SyntaxId, FoldMatch) {
            // The array opens on line 1; the function body on line 0.
            let fold = folds
                .iter()
                .find(|fold| fold.range.start.line.as_usize() == 1)
                .expect("the array is a fold");
            (fold.syntax_id, fold.match_kind)
        };
        let (lhs_array, lhs_match) = collection(&result.lhs_folds);
        let (rhs_array, rhs_match) = collection(&result.rhs_folds);
        assert_eq!(
            (lhs_match, rhs_match),
            (
                FoldMatch::Matched {
                    opposite: rhs_array
                },
                FoldMatch::Matched {
                    opposite: lhs_array
                }
            ),
            "the matcher pairs the arrays"
        );
        let diff = project("a.rs", lhs, rhs);
        let (lhs, rhs) = sources(&diff);
        let (lhs, rhs) = (lhs.unwrap(), rhs.unwrap());
        let lhs_states: BTreeSet<u32> = all(&lhs.regions).iter().map(|r| r.fold_state_id).collect();
        let array = all(&rhs.regions)
            .into_iter()
            .find(|r| matches!(r.node, Node::Fold { .. }) && r.range.start.line == 2)
            .expect("the rhs array is a region");
        assert!(
            !lhs_states.contains(&array.fold_state_id),
            "no lhs region claims the dropped fold's state"
        );
    }

    #[test]
    fn swapped_functions_share_fold_state_but_never_ids() {
        let lhs = "fn a() {\n    one();\n    two();\n}\n\nfn b() {\n    three();\n    four();\n}\n";
        let rhs = "fn b() {\n    three();\n    four();\n}\n\nfn a() {\n    one();\n    two();\n}\n";
        let mut result = DiffResult::from_sources("a.rs", lhs, rhs);
        // Pair each body with the body of the same function, wherever it is:
        // the pairs cross.
        let header = |src: &str, fold: &Fold| {
            src.lines()
                .nth(fold.range.start.line.as_usize())
                .unwrap()
                .to_owned()
        };
        assert_eq!(result.lhs_folds.len(), 2);
        assert_eq!(result.rhs_folds.len(), 2);
        for lhs_index in 0..2 {
            let rhs_index = result
                .rhs_folds
                .iter()
                .position(|fold| header(rhs, fold) == header(lhs, &result.lhs_folds[lhs_index]))
                .unwrap();
            result.lhs_folds[lhs_index].match_kind = FoldMatch::Matched {
                opposite: result.rhs_folds[rhs_index].syntax_id,
            };
            result.rhs_folds[rhs_index].match_kind = FoldMatch::Matched {
                opposite: result.lhs_folds[lhs_index].syntax_id,
            };
        }
        let diff = diff(
            &result,
            Inputs {
                file: &refs(true, true),
                sizes: (lhs.len() as u64, rhs.len() as u64),
            },
        );
        let (lhs_src, rhs_src) = sources(&diff);
        let (lhs_src, rhs_src) = (lhs_src.unwrap(), rhs_src.unwrap());
        let folds = |source: &Source| -> BTreeMap<String, (u32, u32)> {
            all(&source.regions)
                .into_iter()
                .filter(|region| matches!(region.node, Node::Fold { .. }))
                .map(|region| {
                    let line = region.range.start.line as usize;
                    (
                        source.text.lines().nth(line).unwrap().to_owned(),
                        (region.id, region.fold_state_id),
                    )
                })
                .collect()
        };
        let (lhs_folds, rhs_folds) = (folds(lhs_src), folds(rhs_src));
        assert_eq!(lhs_folds.len(), 2, "{lhs_folds:?}");
        assert_eq!(rhs_folds.len(), 2, "{rhs_folds:?}");
        for (name, &(lhs_id, lhs_state)) in &lhs_folds {
            let (_, rhs_state) = rhs_folds[name];
            assert_eq!(
                lhs_state, rhs_state,
                "{name} opens and closes on both sides"
            );
            assert!(rhs_folds.values().all(|&(id, _)| id != lhs_id));
        }
        // Leaves still tile, and every leaf pair still mirrors its splits.
        for source in [lhs_src, rhs_src] {
            assert_tiles(source);
            assert_folds_hold_children(&source.regions);
        }
        let lengths = |source: &Source| -> BTreeMap<u32, u32> {
            leaves(&source.regions)
                .into_iter()
                .map(|leaf| {
                    let (start, end) = leaf.range.lines_spanned();
                    (alignment(leaf), end - start)
                })
                .collect()
        };
        let rhs_lengths = lengths(rhs_src);
        for (id, length) in lengths(lhs_src) {
            if let Some(&other) = rhs_lengths.get(&id) {
                assert_eq!(length, other, "paired leaf {id}");
            }
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
        let (lhs, rhs) = (lhs.unwrap(), rhs.unwrap());
        let (lhs_folds, rhs_folds) = (fold_ids(lhs), fold_ids(rhs));
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
        let (lhs_states, rhs_states) = (fold_states(lhs), fold_states(rhs));
        // Nothing matched the nodes these folds belong to, so no fold pairs.
        let lhs_state_ids: BTreeSet<u32> = lhs_states.values().copied().collect();
        assert!(
            !rhs_states
                .values()
                .any(|state| lhs_state_ids.contains(state)),
            "a fallback's folds are unpaired"
        );
    }

    #[test]
    fn folds_pair_only_where_the_matcher_paired_their_nodes() {
        for options in [
            DiffOptions::default(),
            DiffOptions {
                graph_limit: 1,
                ..DiffOptions::default()
            },
        ] {
            let structural = options.graph_limit != 1;
            let diff = project_with("a.rs", RUST_LHS, RUST_RHS, options);
            let Diff::Text { stats, .. } = &diff else {
                panic!("text diff");
            };
            assert_eq!(stats.fallback.is_none(), structural);
            let (lhs, rhs) = sources(&diff);
            let (lhs, rhs) = (lhs.unwrap(), rhs.unwrap());
            assert_tiles(lhs);
            assert_tiles(rhs);
            let (lhs_folds, rhs_folds) = (fold_states(lhs), fold_states(rhs));
            assert_eq!(lhs_folds.len(), 2);
            assert_eq!(rhs_folds.len(), 3);
            let shared: BTreeSet<u32> = lhs_folds
                .values()
                .filter(|state| rhs_folds.values().any(|other| other == *state))
                .copied()
                .collect();
            if structural {
                // `f` changed its signature and stays paired through the
                // matcher; `keep` is untouched. `added` is rhs-only.
                assert_eq!(
                    lhs_folds[&1], rhs_folds[&1],
                    "f pairs across a changed header"
                );
                assert_eq!(lhs_folds[&7], rhs_folds[&7], "keep pairs");
                assert_eq!(shared.len(), 2);
            } else {
                // The matcher never ran, so every fold is on its own.
                assert!(shared.is_empty(), "a fallback's folds are unpaired");
            }
            assert!(
                !lhs_folds.values().any(|id| *id == rhs_folds[&13]),
                "added is rhs-only"
            );
        }
    }

    #[test]
    fn the_fallback_keeps_folds() {
        let diff = project_with(
            "a.rs",
            RUST_LHS,
            RUST_RHS,
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
        // Nothing is collapsed, so the untouched `keep` is a fold like the
        // changed `f` and the new `added`.
        assert_eq!(bodies, vec![1, 7, 13]);
        assert!(leaves(&rhs.regions)
            .iter()
            .all(|leaf| !leaf.visibility.collapsed && leaf.tags.is_empty()));
    }

    #[test]
    fn leaves_tile_both_sides_and_paired_leaves_share_ids() {
        let lhs = "import os\n\ndef f():\n    x = 1\n    return x\n";
        let rhs =
            "import os\n\ndef f():\n    x = 2\n    return x\n\ndef g():\n    y = 3\n    return y\n";
        let diff = project("a.py", lhs, rhs);
        let (lhs, rhs) = sources(&diff);
        let (lhs, rhs) = (lhs.unwrap(), rhs.unwrap());
        assert_tiles(lhs);
        assert_tiles(rhs);
        assert_folds_hold_children(&lhs.regions);
        assert_folds_hold_children(&rhs.regions);
        // Ids are dense, assigned lhs first, and never shared across sides.
        let lhs_ids: Vec<u32> = all(&lhs.regions).iter().map(|r| r.id).collect();
        let rhs_ids: Vec<u32> = all(&rhs.regions).iter().map(|r| r.id).collect();
        let mut ids: Vec<u32> = lhs_ids.iter().chain(&rhs_ids).copied().collect();
        ids.sort_unstable();
        assert_eq!(ids, (1..=ids.len() as u32).collect::<Vec<_>>());
        assert!(lhs_ids.iter().max() < rhs_ids.iter().min());
        // Leaf alignment ids are dense on their own counter.
        let lhs_leaves: BTreeMap<u32, &Region> = leaves(&lhs.regions)
            .into_iter()
            .map(|leaf| (alignment(leaf), leaf))
            .collect();
        let rhs_leaves: BTreeMap<u32, &Region> = leaves(&rhs.regions)
            .into_iter()
            .map(|leaf| (alignment(leaf), leaf))
            .collect();
        let alignments: BTreeSet<u32> = lhs_leaves
            .keys()
            .chain(rhs_leaves.keys())
            .copied()
            .collect();
        assert_eq!(
            alignments.into_iter().collect::<Vec<_>>(),
            (0..=*lhs_leaves.keys().chain(rhs_leaves.keys()).max().unwrap()).collect::<Vec<_>>()
        );
        let mut paired = 0;
        for (id, lhs_leaf) in &lhs_leaves {
            if let Some(rhs_leaf) = rhs_leaves.get(id) {
                paired += 1;
                assert_eq!(
                    lhs_leaf.range.lines_spanned().1 - lhs_leaf.range.lines_spanned().0,
                    rhs_leaf.range.lines_spanned().1 - rhs_leaf.range.lines_spanned().0,
                    "paired leaves have equal length"
                );
                assert_eq!(lhs_leaf.fold_state_id, rhs_leaf.fold_state_id);
            }
        }
        assert!(paired > 0);
        // Python's body fold covers the body alone: it starts on the line
        // after the `def` line, which the leaf before it holds.
        let new_fold = all(&rhs.regions)
            .into_iter()
            .find(|r| matches!(r.node, Node::Fold { .. }) && r.range.start.line == 7)
            .expect("the added function is a fold");
        // The new function exists on the rhs only: nothing on the lhs opens
        // with it or aligns with its rows.
        let lhs_states: BTreeSet<u32> = all(&lhs.regions).iter().map(|r| r.fold_state_id).collect();
        assert!(!lhs_states.contains(&new_fold.fold_state_id));
        assert!(leaves(std::slice::from_ref(new_fold))
            .into_iter()
            .all(|leaf| !lhs_leaves.contains_key(&alignment(leaf))));
        assert_eq!(new_fold.tags, vec!["body"]);
        assert_eq!(new_fold.visibility.label, "Body");
    }

    #[test]
    fn the_changed_body_is_a_paired_fold_with_a_novel_leaf_inside() {
        let lhs = "def f():\n    a = 1\n    b = 2\n    return a\n";
        let rhs = "def f():\n    a = 1\n    b = 3\n    return a\n";
        let diff = project("a.py", lhs, rhs);
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
        assert_ne!(lhs_fold.id, rhs_fold.id);
        assert_eq!(lhs_fold.fold_state_id, rhs_fold.fold_state_id);
        assert_eq!(lhs_fold.range.lines_spanned(), (1, 4));
        let novel: Vec<_> = leaves(&rhs.regions)
            .into_iter()
            .filter(|leaf| matches!(&leaf.node, Node::Leaf { changed, .. } if !changed.is_empty()))
            .collect();
        assert_eq!(novel.len(), 1);
        assert_eq!(novel[0].range.lines_spanned(), (2, 3));
        let Node::Leaf { changed, .. } = &novel[0].node else {
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
    fn python_body_folds_pair_through_their_block_nodes() {
        // The body fold is the `block` under `def`, opening after the
        // header's `:`. The header changed too; the blocks still pair by
        // node identity, and their folds share one `fold_state_id`.
        let lhs = "def f(a):\n    x = a\n    y = 2\n    return x + y\n";
        let rhs = "def f(a, b):\n    x = a\n    y = 3\n    return x + y\n";
        let result = DiffResult::from_sources("a.py", lhs, rhs);
        assert_eq!(result.lhs_folds.len(), 1);
        assert_eq!(result.rhs_folds.len(), 1);
        let start = |fold: &Fold| {
            (
                fold.range.start.line.as_usize(),
                fold.range.start.byte_column,
            )
        };
        assert_eq!(start(&result.lhs_folds[0]), (0, 9));
        assert_eq!(start(&result.rhs_folds[0]), (0, 12));
        assert_eq!(
            result.lhs_folds[0].match_kind,
            FoldMatch::Matched {
                opposite: result.rhs_folds[0].syntax_id
            }
        );
        assert_eq!(
            result.rhs_folds[0].match_kind,
            FoldMatch::Matched {
                opposite: result.lhs_folds[0].syntax_id
            }
        );
        let diff = project("a.py", lhs, rhs);
        let (lhs, rhs) = sources(&diff);
        let fold = |source: &Source| {
            all(&source.regions)
                .into_iter()
                .find(|r| matches!(r.node, Node::Fold { .. }))
                .expect("the body is a fold")
                .clone()
        };
        let (lhs_fold, rhs_fold) = (fold(lhs.unwrap()), fold(rhs.unwrap()));
        assert_eq!(lhs_fold.fold_state_id, rhs_fold.fold_state_id);
        assert_ne!(lhs_fold.id, rhs_fold.id);
    }

    #[test]
    fn a_fully_new_line_is_painted_whole_and_blank_lines_not_at_all() {
        let diff = project("a.py", "x = 1\n", "x = 1\n\ny = 2\n");
        let (_, rhs) = sources(&diff);
        let changed: Vec<Span> = leaves(&rhs.unwrap().regions)
            .into_iter()
            .filter_map(|leaf| match &leaf.node {
                Node::Leaf { changed, .. } => Some(changed.clone()),
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
    fn a_fold_edge_splits_paired_leaves_on_both_sides() {
        // The unchanged function forces a split on both sides at the same
        // offset, even though only the rhs shifted.
        let lhs = "a = 1\n\ndef f():\n    return 1\n\nz = 1\n";
        let rhs = "a = 2\n\ndef f():\n    return 1\n\nz = 1\n";
        let diff = project("a.py", lhs, rhs);
        let (lhs, rhs) = sources(&diff);
        let (lhs, rhs) = (lhs.unwrap(), rhs.unwrap());
        assert_tiles(lhs);
        assert_tiles(rhs);
        assert_folds_hold_children(&lhs.regions);
        assert_folds_hold_children(&rhs.regions);
        let lhs_leaves: Vec<_> = leaves(&lhs.regions)
            .iter()
            .map(|l| (alignment(l), l.range.lines_spanned()))
            .collect();
        let rhs_leaves: Vec<_> = leaves(&rhs.regions)
            .iter()
            .map(|l| (alignment(l), l.range.lines_spanned()))
            .collect();
        assert_eq!(lhs_leaves, rhs_leaves);
    }

    #[test]
    fn stats_count_changed_lines_and_flag_unsupported_languages() {
        let diff = project("a.py", "x = 1\n", "x = 1 # same\n");
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
        let diff = project("a.unknownext", "x\n", "y\n");
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
        let diff = project("a.py", "", "def f():\n    return 1\n");
        let (lhs, rhs) = sources(&diff);
        assert!(lhs.is_none());
        let rhs = rhs.unwrap();
        assert_tiles(rhs);
        assert!(leaves(&rhs.regions)
            .iter()
            .all(|leaf| !leaf.visibility.collapsed));
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
}
