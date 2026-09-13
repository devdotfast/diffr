//! Projection from the internal `DiffResult` onto the wire types.
//!
//! Leaves come from the full-file row alignment: consecutive rows of one
//! kind (paired unchanged, paired novel, one-sided) become one leaf on each
//! side they touch, and a paired leaf shares its id across sides. Folds
//! come from the per-side fold lists; a fold with a counterpart shares its
//! id too. Leaves are split wherever a fold starts or ends so that every
//! fold's children tile its line span exactly, and a split on one side of
//! a paired leaf is mirrored on the other so paired leaves stay equal in
//! length. Unchanged rows that no hunk shows, which is everything outside
//! the `-U` padding and the enclosing syntax context difftastic already
//! selected, become collapsed leaves tagged `unchanged`. Folds that lie
//! entirely inside such a gap are dropped: they would be hidden anyway, and
//! keeping them would only fragment the gap.
use super::{
    BinaryRef, Diff, FileRef, LineCounts, Node, Pairing, Problem, Region, Source, SourcePos,
    SourceRange, Span, Stats, SyntaxSpan, Visibility,
};
use crate::display::hunks::Hunk;
use crate::display::line_layout::{aligned_rows, novel_lines};
use crate::hash::DftHashMap;
use crate::line_parser;
use crate::lines;
use crate::parse::folds::Fold;
use crate::parse::syntax::{MatchKind, MatchedPos};
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

pub(crate) fn diff(result: &DiffResult, inputs: Inputs<'_>) -> Result<Diff, Problem> {
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
            return Ok(Diff::Binary { sides });
        }
    };
    let (lhs_regions, rhs_regions) = regions(result, lhs_src, rhs_src)?;
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
    Ok(Diff::Text {
        sides,
        stats: stats(result, lhs_src, rhs_src),
    })
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunKind {
    Unchanged,
    Novel,
}

/// A maximal run of aligned rows of one kind before fold splitting.
#[derive(Clone, Debug)]
struct Run {
    kind: RunKind,
    lhs: Option<(usize, usize)>,
    rhs: Option<(usize, usize)>,
    collapsed: bool,
}

impl Run {
    fn len(&self) -> usize {
        let (start, end) = self.lhs.or(self.rhs).expect("a run has a side");
        end - start
    }
}

/// A leaf after splitting. `key` identifies its counterpart on the other
/// side, when it has one.
#[derive(Clone, Debug)]
struct Leaf {
    lines: (usize, usize),
    key: Option<LeafKey>,
    collapsed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum PairKey {
    Leaf(LeafKey),
    Fold(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct LeafKey {
    run: usize,
    piece: usize,
}

struct SideFold<'a> {
    fold: &'a Fold,
    lines: (usize, usize),
    /// Index of the lhs fold this pairs with, on either side.
    pair: Option<usize>,
}

fn regions(
    result: &DiffResult,
    lhs_src: &str,
    rhs_src: &str,
) -> Result<(Vec<Region>, Vec<Region>), Problem> {
    let lhs_lines: Vec<&str> = lhs_src.split_terminator('\n').collect();
    let rhs_lines: Vec<&str> = rhs_src.split_terminator('\n').collect();
    let lhs_novel = novel_lines(&result.lhs_positions);
    let rhs_novel = novel_lines(&result.rhs_positions);
    let rows = aligned_rows(
        (lhs_src, rhs_src),
        (&result.lhs_positions, &result.rhs_positions),
    );
    let (lhs_shown, rhs_shown) = shown_lines(&result.hunks);
    let runs = trim_context(runs(&rows, &lhs_novel, &rhs_novel), &lhs_shown, &rhs_shown);
    let lhs_gaps: Vec<(usize, usize)> = runs
        .iter()
        .filter(|run| run.collapsed)
        .filter_map(|run| run.lhs)
        .collect();
    let rhs_gaps: Vec<(usize, usize)> = runs
        .iter()
        .filter(|run| run.collapsed)
        .filter_map(|run| run.rhs)
        .collect();

    let mut lhs_folds: Vec<SideFold<'_>> = side_folds(&result.lhs_folds, &lhs_lines)
        .into_iter()
        .filter_map(|fold| fit_to_gaps(fold, &lhs_gaps))
        .collect();
    let mut rhs_folds: Vec<SideFold<'_>> = side_folds(&result.rhs_folds, &rhs_lines)
        .into_iter()
        .filter_map(|fold| fit_to_gaps(fold, &rhs_gaps))
        .collect();
    let lhs_splits = split_lines(&lhs_folds);
    let rhs_splits = split_lines(&rhs_folds);
    let (lhs_leaves, rhs_leaves) = split_runs(&runs, &lhs_splits, &rhs_splits);
    pair_folds(
        &mut lhs_folds,
        &mut rhs_folds,
        &rows,
        (&lhs_leaves, &rhs_leaves),
    );

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
    debug_assert_eq!(tree_violation(&lhs), None);
    debug_assert_eq!(tree_violation(&rhs), None);
    Ok((lhs, rhs))
}

/// The first way `regions` fails to be a strict tree, if any: a child
/// outside its parent's byte range, siblings out of order or overlapping,
/// or a fold with no children.
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

/// Rows in order, run-length encoded by kind and side presence.
fn runs(
    rows: &[(Option<usize>, Option<usize>)],
    lhs_novel: &BTreeSet<usize>,
    rhs_novel: &BTreeSet<usize>,
) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for &(lhs, rhs) in rows {
        let kind = match (lhs, rhs) {
            (Some(l), Some(r)) if !lhs_novel.contains(&l) && !rhs_novel.contains(&r) => {
                RunKind::Unchanged
            }
            _ => RunKind::Novel,
        };
        let extends = runs.last().is_some_and(|run| {
            run.kind == kind
                && run.lhs.is_some() == lhs.is_some()
                && run.rhs.is_some() == rhs.is_some()
                && lhs.is_none_or(|l| run.lhs.is_some_and(|(_, end)| end == l))
                && rhs.is_none_or(|r| run.rhs.is_some_and(|(_, end)| end == r))
        });
        if extends {
            let run = runs.last_mut().expect("checked above");
            if let (Some((_, end)), Some(_)) = (&mut run.lhs, lhs) {
                *end += 1;
            }
            if let (Some((_, end)), Some(_)) = (&mut run.rhs, rhs) {
                *end += 1;
            }
        } else {
            runs.push(Run {
                kind,
                lhs: lhs.map(|l| (l, l + 1)),
                rhs: rhs.map(|r| (r, r + 1)),
                collapsed: false,
            });
        }
    }
    runs
}

/// The lines difftastic's hunks display on each side: the `-U` padding
/// around every change plus the enclosing syntax context, such as the
/// header of the function a change sits in.
fn shown_lines(hunks: &[Hunk]) -> (BTreeSet<usize>, BTreeSet<usize>) {
    let mut lhs = BTreeSet::new();
    let mut rhs = BTreeSet::new();
    for hunk in hunks {
        for &(l, r) in &hunk.lines {
            lhs.extend(l.map(|line| line.as_usize()));
            rhs.extend(r.map(|line| line.as_usize()));
        }
    }
    (lhs, rhs)
}

/// Collapsing fewer lines than this saves nothing worth a fold row.
const MIN_GAP: usize = 3;

/// Collapse every maximal stretch of an unchanged run that no hunk shows
/// and that is at least `MIN_GAP` lines long. A file with no change has no
/// hunks, so it becomes one collapsed run.
fn trim_context(
    runs: Vec<Run>,
    lhs_shown: &BTreeSet<usize>,
    rhs_shown: &BTreeSet<usize>,
) -> Vec<Run> {
    let no_change = lhs_shown.is_empty() && rhs_shown.is_empty();
    let mut out: Vec<Run> = Vec::new();
    for run in runs {
        if run.kind != RunKind::Unchanged {
            out.push(run);
            continue;
        }
        let (lhs_start, _) = run.lhs.expect("unchanged runs are paired");
        let (rhs_start, _) = run.rhs.expect("unchanged runs are paired");
        let shown = |offset: usize| {
            lhs_shown.contains(&(lhs_start + offset)) || rhs_shown.contains(&(rhs_start + offset))
        };
        let len = run.len();
        let mut from = 0;
        while from < len {
            let hidden = !shown(from);
            let mut to = from + 1;
            while to < len && shown(to) != hidden {
                to += 1;
            }
            // A file with no change at all is one gap however short.
            let collapsed = hidden && (to - from >= MIN_GAP || no_change);
            let piece = Run {
                kind: RunKind::Unchanged,
                lhs: Some((lhs_start + from, lhs_start + to)),
                rhs: Some((rhs_start + from, rhs_start + to)),
                collapsed,
            };
            match out.last_mut() {
                // Open pieces of one run stay one leaf.
                Some(last)
                    if !collapsed
                        && !last.collapsed
                        && last.kind == RunKind::Unchanged
                        && last.lhs.is_some_and(|(_, end)| end == lhs_start + from) =>
                {
                    last.lhs = Some((last.lhs.expect("paired").0, lhs_start + to));
                    last.rhs = Some((last.rhs.expect("paired").0, rhs_start + to));
                }
                _ => out.push(piece),
            }
            from = to;
        }
    }
    out
}

/// Whether a fold's line span lies entirely inside one collapsed gap.
fn inside_gap(lines: (usize, usize), gaps: &[(usize, usize)]) -> bool {
    gaps.iter()
        .any(|&(start, end)| start <= lines.0 && lines.1 <= end)
}

/// A collapsed gap is never split into two collapsed leaves back to back.
/// A fold entirely inside a gap is dropped: it would be hidden anyway. When
/// a fold edge would cut a gap into two pieces that are each long enough to
/// collapse, the fold gives way: a fold whose header starts inside such a
/// gap is dropped (its header would be hidden), and a fold that ends inside
/// one extends to the gap's end, so the gap stays one leaf inside it. Any
/// ancestor ending in the same gap extends to the same line, so nesting
/// holds. A cut leaving a sliver shorter than `MIN_GAP` needs nothing: the
/// sliver stays open. Gaps are aligned runs, so both sides agree.
fn fit_to_gaps<'a>(mut fold: SideFold<'a>, gaps: &[(usize, usize)]) -> Option<SideFold<'a>> {
    if inside_gap(fold.lines, gaps) {
        return None;
    }
    let (start, end) = fold.lines;
    let splits = |gap_start: usize, cut: usize, gap_end: usize| {
        gap_start < cut && cut < gap_end && cut - gap_start >= MIN_GAP && gap_end - cut >= MIN_GAP
    };
    if gaps
        .iter()
        .any(|&(gap_start, gap_end)| splits(gap_start, start, gap_end.min(end)))
    {
        return None;
    }
    if let Some(&(_, gap_end)) = gaps
        .iter()
        .find(|&&(gap_start, gap_end)| splits(gap_start.max(start), end, gap_end))
    {
        fold.lines = (start, gap_end);
    }
    Some(fold)
}

fn line_span(range: &lines::SourceRange, line_count: usize) -> (usize, usize) {
    let start = range.start.line.as_usize();
    let end = if range.end.byte_column == 0 {
        range.end.line.as_usize()
    } else {
        range.end.line.as_usize() + 1
    };
    (
        start.min(line_count),
        end.min(line_count).max(start.min(line_count)),
    )
}

fn side_folds<'a>(folds: &'a [Fold], lines: &[&str]) -> Vec<SideFold<'a>> {
    let spans: Vec<(usize, usize)> = folds
        .iter()
        .map(|fold| line_span(&fold.range, lines.len()))
        .collect();
    folds
        .iter()
        .zip(nested_spans(&spans))
        // A fold on a single line hides nothing; it is not a region.
        .filter_map(|(fold, span)| {
            span.map(|lines| SideFold {
                fold,
                lines,
                pair: None,
            })
        })
        .collect()
}

/// Make fold line spans a strict tree: nested or disjoint, never crossing.
///
/// Leaves tile whole lines, so two folds can share a line only if one
/// contains the other. The parser can hand over folds that cross on one
/// line, such as a collection whose closer sits on the line that opens the
/// next body (`for x in [ … ] {`). The rule: the earlier fold gives the
/// shared line to the later one, so its span ends where the later fold
/// starts. A span left with fewer than two lines hides nothing and is
/// dropped (`None`). Output is in input order.
fn nested_spans(spans: &[(usize, usize)]) -> Vec<Option<(usize, usize)>> {
    let mut order: Vec<usize> = (0..spans.len()).collect();
    order.sort_by_key(|&index| (spans[index].0, std::cmp::Reverse(spans[index].1)));
    let mut out: Vec<Option<(usize, usize)>> = spans.iter().map(|&span| Some(span)).collect();
    let mut open: Vec<usize> = Vec::new();
    let closed = |out: &[Option<(usize, usize)>], top: usize, start: usize| {
        out[top].is_none_or(|(_, top_end)| top_end <= start)
    };
    for index in order {
        let (start, end) = spans[index];
        while open.last().is_some_and(|&top| closed(&out, top, start)) {
            open.pop();
        }
        // Every open fold that ends before this one does crosses it: clip
        // each to hand over the shared lines.
        for &top in open.iter().rev() {
            let Some((top_start, top_end)) = out[top] else {
                continue;
            };
            if top_end >= end {
                break;
            }
            out[top] = (start > top_start).then_some((top_start, start));
        }
        while open.last().is_some_and(|&top| closed(&out, top, start)) {
            open.pop();
        }
        open.push(index);
    }
    for span in &mut out {
        if span.is_some_and(|(start, end)| end - start < 2) {
            *span = None;
        }
    }
    out
}

/// Two folds correspond when the line alignment pairs their header lines
/// and their tags agree. A changed signature is still a paired row, so a
/// function whose header was edited keeps its counterpart. The rule is the
/// same whichever engine produced the alignment: the structural engine
/// pairs rows through matched tokens, the text diff through its line
/// alignment. Folds sharing a header line on one side, such as a body and
/// a collection opened on the same line, pair in order of span length.
/// Assign fold correspondence. Rule 1: header lines aligned in the row
/// table and equal tags. Rule 2, for a lhs fold rule 1 left unpaired: the
/// rhs fold with equal tags that contains the counterpart of a paired leaf
/// inside it, choosing the candidate holding the most paired lines (ties
/// by the nearest header in row order), provided that fold is free and the
/// pair does not cross an existing pair. This is what keeps a function
/// whose signature was reflowed onto more lines paired with itself.
fn pair_folds(
    lhs_folds: &mut [SideFold<'_>],
    rhs_folds: &mut [SideFold<'_>],
    rows: &[(Option<usize>, Option<usize>)],
    (lhs_leaves, rhs_leaves): (&[Leaf], &[Leaf]),
) {
    let aligned: DftHashMap<usize, usize> = rows
        .iter()
        .filter_map(|&(lhs, rhs)| Some((lhs?, rhs?)))
        .collect();
    let mut rhs_by_header: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, fold) in rhs_folds.iter().enumerate() {
        rhs_by_header.entry(fold.lines.0).or_default().push(index);
    }
    let span = |fold: &SideFold<'_>| fold.lines.1 - fold.lines.0;
    for candidates in rhs_by_header.values_mut() {
        candidates.sort_by_key(|&index| std::cmp::Reverse(span(&rhs_folds[index])));
    }
    let mut lhs_by_header: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, fold) in lhs_folds.iter().enumerate() {
        lhs_by_header.entry(fold.lines.0).or_default().push(index);
    }
    for (header, mut own) in lhs_by_header {
        let Some(other) = aligned
            .get(&header)
            .and_then(|line| rhs_by_header.get(line))
        else {
            continue;
        };
        own.sort_by_key(|&index| std::cmp::Reverse(span(&lhs_folds[index])));
        for lhs_index in own {
            let tags = &lhs_folds[lhs_index].fold.tags;
            let Some(&rhs_index) = other.iter().find(|&&index| {
                rhs_folds[index].pair.is_none() && rhs_folds[index].fold.tags == *tags
            }) else {
                continue;
            };
            lhs_folds[lhs_index].pair = Some(lhs_index);
            rhs_folds[rhs_index].pair = Some(lhs_index);
        }
    }
    pair_folds_by_content(lhs_folds, rhs_folds, rows, lhs_leaves, rhs_leaves);
}

/// Rule 2 of `pair_folds`: pair through a shared leaf rather than the
/// header line. Paired leaves carry the same key on both sides.
fn pair_folds_by_content(
    lhs_folds: &mut [SideFold<'_>],
    rhs_folds: &mut [SideFold<'_>],
    rows: &[(Option<usize>, Option<usize>)],
    lhs_leaves: &[Leaf],
    rhs_leaves: &[Leaf],
) {
    // Row index of each side's line, for crossing checks and tie-breaks.
    let mut row_of_lhs: DftHashMap<usize, usize> = DftHashMap::default();
    let mut row_of_rhs: DftHashMap<usize, usize> = DftHashMap::default();
    for (index, &(lhs, rhs)) in rows.iter().enumerate() {
        if let Some(line) = lhs {
            row_of_lhs.entry(line).or_insert(index);
        }
        if let Some(line) = rhs {
            row_of_rhs.entry(line).or_insert(index);
        }
    }
    let rhs_leaf_by_key: DftHashMap<LeafKey, &Leaf> = rhs_leaves
        .iter()
        .filter_map(|leaf| leaf.key.map(|key| (key, leaf)))
        .collect();
    let contains = |fold: &SideFold<'_>, leaf: &Leaf| {
        fold.lines.0 <= leaf.lines.0 && leaf.lines.1 <= fold.lines.1
    };
    // Existing pairs as (lhs header row, rhs header row), to forbid crossings.
    let mut pairs: Vec<(usize, usize)> = rhs_folds
        .iter()
        .filter_map(|rhs| {
            let lhs = &lhs_folds[rhs.pair?];
            Some((row_of_lhs[&lhs.lines.0], row_of_rhs[&rhs.lines.0]))
        })
        .collect();
    #[allow(clippy::needless_range_loop)]
    for lhs_index in 0..lhs_folds.len() {
        if lhs_folds[lhs_index].pair.is_some() {
            continue;
        }
        let lhs_fold = &lhs_folds[lhs_index];
        // Paired lines per candidate rhs fold.
        let mut score: DftHashMap<usize, usize> = DftHashMap::default();
        for leaf in lhs_leaves.iter().filter(|leaf| contains(lhs_fold, leaf)) {
            let Some(partner) = leaf.key.and_then(|key| rhs_leaf_by_key.get(&key)) else {
                continue;
            };
            for (rhs_index, rhs_fold) in rhs_folds.iter().enumerate() {
                if rhs_fold.pair.is_none()
                    && rhs_fold.fold.tags == lhs_fold.fold.tags
                    && contains(rhs_fold, partner)
                {
                    *score.entry(rhs_index).or_default() += partner.lines.1 - partner.lines.0;
                }
            }
        }
        let lhs_row = row_of_lhs[&lhs_fold.lines.0];
        let mut candidates: Vec<(usize, usize)> = score.into_iter().collect();
        candidates.sort_by_key(|&(rhs_index, lines)| {
            let rhs_row = row_of_rhs[&rhs_folds[rhs_index].lines.0];
            (
                std::cmp::Reverse(lines),
                rhs_row.abs_diff(lhs_row),
                rhs_index,
            )
        });
        let Some(&(rhs_index, _)) = candidates.iter().find(|&&(rhs_index, _)| {
            let rhs_row = row_of_rhs[&rhs_folds[rhs_index].lines.0];
            !pairs.iter().any(|&(a, b)| (a < lhs_row) != (b < rhs_row))
        }) else {
            continue;
        };
        lhs_folds[lhs_index].pair = Some(lhs_index);
        rhs_folds[rhs_index].pair = Some(lhs_index);
        pairs.push((lhs_row, row_of_rhs[&rhs_folds[rhs_index].lines.0]));
    }
}

fn split_lines(folds: &[SideFold<'_>]) -> BTreeSet<usize> {
    folds
        .iter()
        .flat_map(|fold| [fold.lines.0, fold.lines.1])
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
        let mut offsets: BTreeSet<usize> = BTreeSet::new();
        if let Some((start, end)) = run.lhs {
            offsets.extend(lhs_splits.range(start + 1..end).map(|line| line - start));
        }
        if let Some((start, end)) = run.rhs {
            offsets.extend(rhs_splits.range(start + 1..end).map(|line| line - start));
        }
        let paired = run.lhs.is_some() && run.rhs.is_some();
        let mut at = 0;
        for (piece, cut) in offsets.into_iter().chain([len]).enumerate() {
            let key = paired.then_some(LeafKey { run: index, piece });
            // A sliver cut off a gap by a fold edge stays open: the same
            // offsets apply to both sides, so paired pieces agree.
            let collapsed = run.collapsed && (cut - at >= MIN_GAP || cut - at == len);
            if let Some((start, _)) = run.lhs {
                lhs_leaves.push(Leaf {
                    lines: (start + at, start + cut),
                    key,
                    collapsed,
                });
            }
            if let Some((start, _)) = run.rhs {
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

#[derive(Default)]
struct Ids {
    next: u32,
    shared: DftHashMap<PairKey, u32>,
}

impl Ids {
    fn get(&mut self, key: Option<PairKey>) -> u32 {
        if let Some(key) = key {
            if let Some(&id) = self.shared.get(&key) {
                return id;
            }
        }
        let id = self.next;
        self.next += 1;
        if let Some(key) = key {
            self.shared.insert(key, id);
        }
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

/// Nest folds and leaves by containment on line spans and assign ids in
/// document order.
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
                let id = ids.get(fold.pair.map(PairKey::Fold));
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
    let id = ids.get(leaf.key.map(PairKey::Leaf));
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
    use crate::parse::folds::FoldMatch;

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
        .unwrap()
    }

    fn sources(diff: &Diff) -> (Option<&Source>, Option<&Source>) {
        match diff {
            Diff::Text { sides, .. } => (sides.lhs(), sides.rhs()),
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
    fn crossing_folds_give_the_shared_line_to_the_later_fold() {
        // A collection closing on line 5 where a body opens: `for x in [ … ] {`.
        let spans = [(0, 6), (5, 9), (7, 8)];
        assert_eq!(
            nested_spans(&spans),
            vec![Some((0, 5)), Some((5, 9)), None],
            "the earlier fold ends where the later starts; one-line spans go"
        );
        // Clipping cascades through every open ancestor that would cross.
        assert_eq!(
            nested_spans(&[(0, 10), (2, 6), (4, 8)]),
            vec![Some((0, 10)), Some((2, 4)), Some((4, 8))]
        );
        // An ancestor that is clipped down to its header line disappears.
        assert_eq!(nested_spans(&[(3, 5), (4, 9)]), vec![None, Some((4, 9))]);
        // Nested and disjoint spans are untouched, whatever their input order.
        assert_eq!(
            nested_spans(&[(5, 9), (0, 4), (1, 3)]),
            vec![Some((5, 9)), Some((0, 4)), Some((1, 3))]
        );
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
    fn folds_pair_through_the_line_alignment_on_either_engine() {
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
            // `f` changed its signature; its header rows still align, so the
            // bodies stay paired. `keep` is untouched. `added` is rhs-only.
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
    fn the_alignment_rule_agrees_with_the_matcher_where_headers_align() {
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
            let matcher_paired = matches!(fold.match_kind, FoldMatch::Unchanged { .. });
            assert_eq!(rhs_ids.contains(&id), matcher_paired, "{:?}", fold.range);
        }
    }

    #[test]
    fn a_reflowed_signature_pairs_through_a_paired_inner_leaf() {
        // The signature moves onto three lines, so the header rows no
        // longer align, but the body is untouched: still the same function.
        let lhs =
            "fn f(a: u32, b: u32) -> u32 {\n    let x = a + b;\n    let y = x * 2;\n    x + y\n}\n";
        let rhs = "fn f(\n    a: u32,\n    b: u32,\n) -> u32 {\n    let x = a + b;\n    let y = x * 2;\n    x + y\n}\n";
        let diff = project("a.rs", lhs, rhs, 3);
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
    fn content_pairing_picks_the_candidate_with_the_most_paired_lines() {
        // Both rhs functions have reflowed signatures and both hold a line
        // `f` also holds (`let x = ...`), but only `g` holds `f`'s long
        // body. Order is preserved so nothing crosses.
        let lhs = "fn h(b: u32) -> u32 {\n    let x = b;\n    b\n}\n\nfn f(a: u32) -> u32 {\n    let x = a;\n    let y = x * 2;\n    let z = y * 3;\n    x + y + z\n}\n";
        let rhs = "fn h(\n    b: u32,\n) -> u32 {\n    let x = b;\n    b\n}\n\nfn g(\n    a: u32,\n) -> u32 {\n    let x = a;\n    let y = x * 2;\n    let z = y * 3;\n    x + y + z\n}\n";
        let diff = project("a.rs", lhs, rhs, 3);
        let (lhs_src, rhs_src) = sources(&diff);
        let (lhs_folds, rhs_folds) = (fold_ids(lhs_src.unwrap()), fold_ids(rhs_src.unwrap()));
        // Fold headers sit on the `{` line: lhs h=0, f=5; rhs h=2, g=9.
        assert_eq!(lhs_folds[&0], rhs_folds[&2], "h pairs with h");
        assert_eq!(lhs_folds[&5], rhs_folds[&9], "f pairs with g by its body");
    }

    #[test]
    fn content_pairing_never_crosses_an_existing_pair() {
        // `keep` pairs by header on both sides. `f` moved below `keep` on
        // the rhs with a reflowed signature; pairing it would cross `keep`,
        // so it stays one-sided on each side.
        let lhs = "fn f(a: u32) -> u32 {\n    let x = a + 1;\n    let y = x * 2;\n    x + y\n}\n\nfn keep() -> u32 {\n    let k = 1;\n    let m = 2;\n    k + m\n}\n";
        let rhs = "fn keep() -> u32 {\n    let k = 1;\n    let m = 2;\n    k + m\n}\n\nfn f(\n    a: u32,\n) -> u32 {\n    let x = a + 1;\n    let y = x * 2;\n    x + y\n}\n";
        let diff = project("a.rs", lhs, rhs, 3);
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
        )
        .unwrap();
        match diff {
            Diff::Binary { sides } => {
                assert_eq!(sides.lhs().unwrap().size, 3);
                assert_eq!(sides.rhs().unwrap().size, 5);
            }
            Diff::Text { .. } => panic!("binary"),
        }
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
