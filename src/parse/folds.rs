//! Fold metadata is attached during parsing; pairing reuses syntax identity.
use super::query::node_range;
use crate::config::query::AnnotationQuery;
use crate::diff::changes::{ChangeKind, ChangeMap};
use crate::display::line_layout::MIN_GAP;
use crate::hash::{DftHashMap, DftHashSet};
use crate::lines::{SourcePosition, SourceRange};
use crate::parse::syntax::{FoldMetadata, Syntax, SyntaxId};
use std::collections::BTreeSet;
use streaming_iterator::StreamingIterator as _;
use tree_sitter::{QueryCursor, Tree};

#[derive(Debug)]
pub(crate) struct Fold {
    pub(crate) tags: Vec<String>,
    /// Source on this side; may span multiple syntax nodes.
    pub(crate) range: SourceRange,
    /// The syntax node the fold was built on. Engine-internal identity:
    /// syntax ids are unique across both sides of a file and never reach
    /// the wire, where the projection numbers regions itself.
    pub(crate) syntax_id: SyntaxId,
    /// The `syntax_id` of the corresponding fold on the other side, when
    /// there is one. Pairs are mutual: the partner records this fold back.
    pub(crate) partner: Option<SyntaxId>,
    /// Text shown in place of the source, including supplied pseudocode.
    pub(crate) placeholder: String,
    /// Longer replacement text supplied by a configured fold hook.
    pub(crate) summary: Option<String>,
}

impl Fold {
    /// The fold on the other side that pairs with this one, if any.
    pub(crate) fn counterpart<'a>(&self, other_side: &'a [Fold]) -> Option<&'a Fold> {
        let partner = self.partner?;
        other_side.iter().find(|other| other.syntax_id == partner)
    }
}

/// Interpret configurable fold captures in their own query traversal.
pub(crate) fn classify(
    tree: &Tree,
    src: &str,
    compiled: Option<&AnnotationQuery>,
) -> DftHashMap<usize, FoldMetadata> {
    let mut kinds = DftHashMap::default();
    let Some(compiled) = compiled else {
        return kinds;
    };
    let query = &compiled.query;
    let mut ambiguous_folds = DftHashSet::default();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), src.as_bytes());
    while let Some(matched) = matches.next() {
        let pattern = &compiled.patterns[matched.pattern_index];
        for fold in matched
            .captures
            .iter()
            .filter(|capture| query.capture_names()[capture.index as usize] == "fold")
        {
            let capture = |name| {
                matched
                    .captures
                    .iter()
                    .rev()
                    .find(|capture| query.capture_names()[capture.index as usize] == name)
            };
            let region = match (capture("fold.open"), capture("fold.close")) {
                (Some(open), Some(close))
                    if open.node.start_byte() >= fold.node.start_byte()
                        && close.node.end_byte() <= fold.node.end_byte()
                        && open.node.end_byte() <= close.node.start_byte() =>
                {
                    SourceRange {
                        start: node_range(open.node).end,
                        end: node_range(close.node).start,
                    }
                }
                (None, None) => node_range(fold.node),
                _ => continue,
            };
            if region.start == region.end || ambiguous_folds.contains(&fold.node.id()) {
                continue;
            }
            let metadata = kinds.entry(fold.node.id()).or_insert_with(|| FoldMetadata {
                tags: Vec::new(),
                range_override: Some(region),
            });
            if metadata.range_override != Some(region) {
                kinds.remove(&fold.node.id());
                ambiguous_folds.insert(fold.node.id());
                continue;
            }
            metadata.tags.extend(pattern.tags.iter().cloned());
            metadata.tags.sort();
            metadata.tags.dedup();
        }
    }
    kinds
}

/// Lists already retain the two edges of their interior, even without delimiters.
pub(crate) fn interior_range(
    open: &[line_numbers::SingleLineSpan],
    close: &[line_numbers::SingleLineSpan],
) -> SourceRange {
    let open = open.last().expect("list opening position");
    let close = close.first().expect("list closing position");
    SourceRange {
        start: SourcePosition {
            line: open.line,
            byte_column: open.end_col as usize,
        },
        end: SourcePosition {
            line: close.line,
            byte_column: close.start_col as usize,
        },
    }
}

fn range(node: &Syntax<'_>) -> Option<SourceRange> {
    let metadata = node.info().fold.borrow();
    let metadata = metadata.as_ref()?;
    let region = match (metadata.range_override, node) {
        (Some(region), _) => region,
        (
            None,
            Syntax::List {
                open_position,
                close_position,
                ..
            },
        ) => interior_range(open_position, close_position),
        (None, Syntax::Atom { position, .. }) => {
            let first = position.first().expect("atom start");
            let last = position.last().expect("atom end");
            SourceRange {
                start: SourcePosition {
                    line: first.line,
                    byte_column: first.start_col as usize,
                },
                end: SourcePosition {
                    line: last.line,
                    byte_column: last.end_col as usize,
                },
            }
        }
    };
    if (region.start.line, region.start.byte_column) >= (region.end.line, region.end.byte_column) {
        return None;
    }
    Some(region)
}

/// Half-open whole lines a fold touches, clamped to the file.
pub(crate) fn line_span(fold: &Fold, line_count: usize) -> (usize, usize) {
    let range = &fold.range;
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

/// Make fold line spans a strict tree: nested or disjoint, never crossing.
///
/// Leaves tile whole lines, so two folds can share a line only if one
/// contains the other. The parser can hand over folds that cross on one
/// line, such as a collection whose closer sits on the line that opens the
/// next body (`for x in [ … ] {`). The rule: the earlier fold gives the
/// shared line to the later one, so its span ends where the later fold
/// starts. A span left with fewer than two lines hides nothing and is
/// dropped (`None`). Output is in input order.
pub(crate) fn nested_spans(spans: &[(usize, usize)]) -> Vec<Option<(usize, usize)>> {
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

/// Every line where a span starts or ends: where leaves must split.
pub(crate) fn split_lines(spans: impl IntoIterator<Item = (usize, usize)>) -> BTreeSet<usize> {
    spans
        .into_iter()
        .flat_map(|(start, end)| [start, end])
        .collect()
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
pub(crate) fn fit_to_gaps(span: (usize, usize), gaps: &[(usize, usize)]) -> Option<(usize, usize)> {
    if inside_gap(span, gaps) {
        return None;
    }
    let (start, end) = span;
    let splits = |gap_start: usize, cut: usize, gap_end: usize| {
        gap_start < cut && cut < gap_end && cut - gap_start >= MIN_GAP && gap_end - cut >= MIN_GAP
    };
    if gaps
        .iter()
        .any(|&(gap_start, gap_end)| splits(gap_start, start, gap_end.min(end)))
    {
        return None;
    }
    match gaps
        .iter()
        .find(|&&(gap_start, gap_end)| splits(gap_start.max(start), end, gap_end))
    {
        Some(&(_, gap_end)) => Some((start, gap_end)),
        None => Some(span),
    }
}

/// The node the matcher paired `node` with, when the pairing is mutual.
///
/// Every matcher step records a pair on both nodes, but the nested slider
/// fixes run one side at a time and move a pair's record from a list onto
/// its child (or parent) on that side only: afterwards the child names the
/// opposite list, while the opposite list still names the original. Such a
/// one-way record is not a pair.
pub(crate) fn partner<'a>(
    node: &Syntax<'_>,
    change: ChangeKind<'a>,
    change_map: &ChangeMap<'a>,
) -> Option<&'a Syntax<'a>> {
    let other = match change {
        ChangeKind::Unchanged(other)
        | ChangeKind::ReplacedComment(_, other)
        | ChangeKind::ReplacedString(_, other) => other,
        ChangeKind::IgnoredPunctuation | ChangeKind::Novel => return None,
    };
    let back = match change_map
        .get(other)
        .expect("the matcher records a change on every node")
    {
        ChangeKind::Unchanged(back)
        | ChangeKind::ReplacedComment(_, back)
        | ChangeKind::ReplacedString(_, back) => back,
        ChangeKind::IgnoredPunctuation | ChangeKind::Novel => return None,
    };
    (back.id() == node.id()).then_some(other)
}

/// Build the fold annotated on `node`, if any, recording the `partner` the
/// matcher paired `node` with (see `partner`).
pub(crate) fn project(node: &Syntax<'_>, partner: Option<&Syntax<'_>>) -> Option<Fold> {
    let own = node.info().fold.borrow().clone()?;
    let own_range = range(node)?;
    Some(Fold {
        tags: own.tags.clone(),
        range: own_range,
        syntax_id: node.id(),
        partner: partner.map(Syntax::id),
        placeholder: own
            .tags
            .first()
            .map(|tag| {
                let mut chars = tag.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                    .unwrap_or_default()
            })
            .unwrap_or_else(|| "…".into()),
        summary: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff::sliders::fix_all_sliders;
    use crate::parse::guess_language::Language;
    use crate::parse::syntax::{init_all_info, AtomKind};
    use line_numbers::SingleLineSpan;
    use typed_arena::Arena;

    #[test]
    fn a_pair_moved_by_a_one_sided_slider_fix_is_not_a_pair() {
        let span = |line: u32| {
            vec![SingleLineSpan {
                line: line.into(),
                start_col: 0,
                end_col: 1,
            }]
        };
        let arena = Arena::new();
        // lhs `((x))`, rhs `(x)`: the matcher pairs the outer lists, and the
        // lhs inner list is novel.
        let lhs_x = Syntax::new_atom(&arena, span(0), "x".to_owned(), AtomKind::Normal);
        let lhs_inner = Syntax::new_list(&arena, "(", span(0), vec![lhs_x], ")", span(0));
        let lhs_outer = Syntax::new_list(&arena, "(", span(0), vec![lhs_inner], ")", span(0));
        let rhs_x = Syntax::new_atom(&arena, span(1), "x".to_owned(), AtomKind::Normal);
        let rhs_outer = Syntax::new_list(&arena, "(", span(1), vec![rhs_x], ")", span(1));
        init_all_info(&[lhs_outer], &[rhs_outer]);
        let mut change_map = ChangeMap::default();
        change_map.insert(lhs_outer, ChangeKind::Unchanged(rhs_outer));
        change_map.insert(rhs_outer, ChangeKind::Unchanged(lhs_outer));
        change_map.insert(lhs_inner, ChangeKind::Novel);
        change_map.insert(lhs_x, ChangeKind::Unchanged(rhs_x));
        change_map.insert(rhs_x, ChangeKind::Unchanged(lhs_x));
        fn partner_of<'a>(node: &'a Syntax<'a>, change_map: &ChangeMap<'a>) -> Option<SyntaxId> {
            partner(node, change_map.get(node).unwrap(), change_map).map(Syntax::id)
        }
        assert_eq!(partner_of(lhs_outer, &change_map), Some(rhs_outer.id()));
        assert_eq!(partner_of(rhs_outer, &change_map), Some(lhs_outer.id()));

        // Lisp prefers the outer delimiter: on the lhs only, the record moves
        // from the outer list onto the inner one.
        fix_all_sliders(Language::EmacsLisp, &[lhs_outer], &mut change_map);
        assert_eq!(
            change_map.get(lhs_inner),
            Some(ChangeKind::Unchanged(rhs_outer))
        );
        assert_eq!(
            change_map.get(rhs_outer),
            Some(ChangeKind::Unchanged(lhs_outer))
        );
        assert_eq!(partner_of(lhs_inner, &change_map), None);
        assert_eq!(partner_of(rhs_outer, &change_map), None);
        assert_eq!(partner_of(lhs_outer, &change_map), None);
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
}
