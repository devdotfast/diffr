//! Fold metadata is attached during parsing; pairing reuses syntax identity.
//!
//! A fold is a region of a file, not a syntax node: two captures that cover
//! the same lines — from one query, or from two plugins' queries — are one
//! fold whose tags are the union of theirs (see [`merge_spans`]). The merged
//! fold is owned by the innermost node that produced it, the one whose
//! extent really is that region, and carries that node's `syntax_id` and its
//! pairing, so alignment follows the node the matcher paired.
use super::query::node_range;
use crate::config::query::AnnotationQuery;
use crate::diff::changes::{ChangeKind, ChangeMap};
use crate::hash::DftHashMap;
use crate::lines::{SourcePosition, SourceRange};
use crate::parse::syntax::{FoldMetadata, Syntax, SyntaxId};
use hashbrown::hash_map::Entry;
use std::collections::BTreeSet;
use streaming_iterator::StreamingIterator as _;
use tree_sitter::{QueryCursor, Tree};

#[derive(Debug)]
pub(crate) struct Fold {
    pub(crate) tags: Vec<String>,
    /// Source on this side; may span multiple syntax nodes.
    pub(crate) range: SourceRange,
    /// The syntax node the fold was built on. Engine-internal identity:
    /// syntax ids are unique across both sides of a file and never reach the
    /// wire, where the projection numbers regions itself.
    pub(crate) syntax_id: SyntaxId,
    pub(crate) match_kind: FoldMatch,
    /// Text shown in place of the source.
    pub(crate) placeholder: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FoldMatch {
    /// The node of the mutually matched fold on the other side, whose
    /// contents may differ. It records this fold back as its own `opposite`.
    Matched {
        opposite: SyntaxId,
    },
    Novel,
}

/// Two query patterns captured the same syntax node with different fold
/// ranges. Which range wins would depend on query order, so the file is not
/// diffed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Conflict {
    /// 0-based line the node starts on.
    pub(crate) line: usize,
    /// The node's tree-sitter kind, such as `function_item`.
    pub(crate) kind: String,
    /// The two patterns, by index in the language's fold query, sorted. The
    /// query is one text here, so an index is all a pattern can be named by.
    pub(crate) patterns: (usize, usize),
}

/// Interpret configurable fold captures in their own query traversal.
///
/// Captures of the same node merge their tags when their ranges agree; when
/// they disagree the result is a [`Conflict`].
pub(crate) fn classify(
    tree: &Tree,
    src: &str,
    compiled: Option<&AnnotationQuery>,
) -> Result<DftHashMap<usize, FoldMetadata>, Conflict> {
    let mut kinds: DftHashMap<usize, (FoldMetadata, usize)> = DftHashMap::default();
    let Some(compiled) = compiled else {
        return Ok(DftHashMap::default());
    };
    let query = &compiled.query;
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
                // No closing delimiter: the fold runs to the end of its
                // node, from an opening token that may precede the node,
                // such as the `:` ahead of a Python block.
                (Some(open), None) if open.node.end_byte() <= fold.node.end_byte() => SourceRange {
                    start: node_range(open.node).end,
                    end: node_range(fold.node).end,
                },
                (None, None) => node_range(fold.node),
                _ => continue,
            };
            if region.start == region.end {
                continue;
            }
            let (metadata, first_pattern) = kinds.entry(fold.node.id()).or_insert((
                FoldMetadata {
                    tags: Vec::new(),
                    range_override: Some(region),
                },
                matched.pattern_index,
            ));
            if metadata.range_override != Some(region) {
                let mut patterns = [*first_pattern, matched.pattern_index];
                patterns.sort();
                return Err(Conflict {
                    line: fold.node.start_position().row,
                    kind: fold.node.kind().to_owned(),
                    patterns: (patterns[0], patterns[1]),
                });
            }
            metadata.tags.extend(pattern.tags.iter().cloned());
            metadata.tags.sort();
            metadata.tags.dedup();
        }
    }
    Ok(kinds
        .into_iter()
        .map(|(id, (metadata, _))| (id, metadata))
        .collect())
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

fn range(node: &Syntax<'_>, metadata: &FoldMetadata) -> Option<SourceRange> {
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

/// Half-open whole lines a fold hides, clamped to the file.
///
/// A fold covers only the lines it holds whole, so collapsing it hides
/// exactly those and no code the reader needs. A fold that opens mid-line —
/// after the `{`, `:` or `(` of a header — starts on the line after: the
/// header is code the fold does not cover, and belongs to the leaf before
/// it. A fold that opens at the start of its line's text, such as a comment
/// block or an import, starts on that line. The end is the mirror: the line
/// a fold closes on is inside it only when nothing but whitespace follows
/// the close, so `] {`, which ends a collection and opens the body after it,
/// is in neither fold.
pub(crate) fn line_span(fold: &Fold, lines: &[&str]) -> (usize, usize) {
    let line_count = lines.len();
    let range = &fold.range;
    let first = range.start.line.as_usize();
    // Code before the fold opens on its first line, or after it closes on
    // its last, is code the fold does not cover: the line stays outside.
    let leading = lines.get(first).is_some_and(|line| {
        let column = range.start.byte_column.min(line.len());
        !line[..column].trim().is_empty()
    });
    let start = if leading { first + 1 } else { first };
    let last = range.end.line.as_usize();
    let trailing = lines.get(last).is_some_and(|line| {
        let column = range.end.byte_column.min(line.len());
        !line[column..].trim().is_empty()
    });
    let end = if trailing { last } else { last + 1 };
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

/// Every fold in one parsed side, each unpaired: the parse's folds survive
/// when the matcher does not run, with nothing to pair them with.
pub(crate) fn unmatched(nodes: &[&Syntax<'_>], folds: &mut Vec<Fold>) {
    for node in nodes {
        folds.extend(project(node, None));
        if let Syntax::List { children, .. } = node {
            unmatched(children, folds);
        }
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

/// The fold annotated on `node`, matched with the fold of the `partner` the
/// matcher paired `node` with (see `partner`). A fold is owned by its syntax
/// node, so two folds align exactly when the matcher paired their nodes.
pub(crate) fn project(node: &Syntax<'_>, partner: Option<&Syntax<'_>>) -> Option<Fold> {
    let own = node.info().fold.borrow();
    let metadata = own.as_ref()?;
    let own_range = range(node, metadata)?;
    // A fold whose range is empty is no fold, on either side.
    let opposite = partner.filter(|partner| {
        partner
            .info()
            .fold
            .borrow()
            .as_ref()
            .is_some_and(|metadata| range(partner, metadata).is_some())
    });
    Some(Fold {
        tags: metadata.tags.clone(),
        range: own_range,
        syntax_id: node.id(),
        match_kind: match opposite {
            Some(partner) => FoldMatch::Matched {
                opposite: partner.id(),
            },
            None => FoldMatch::Novel,
        },
        placeholder: metadata
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
    })
}

/// Fold the folds of one side that cover the same lines into one.
///
/// Two captures — one query's, or two plugins' — can fold the same region
/// of the file from different syntax nodes, such as a Rust `Self { … }` as
/// a function's tail expression and the `{ … }` field list inside it. That
/// is one fold: its tags are the union of theirs, sorted and deduplicated.
/// (Two ranges on the *same* node are a query conflict instead, and the
/// file is not diffed; see [`classify`].)
///
/// The merged fold is owned by the innermost node that produced it — the
/// one whose range lies inside the others', and among equal ranges the one
/// the preorder walk reached last. It keeps that node's `syntax_id` and its
/// pairing, so alignment follows the node whose extent is the fold's
/// region. `folds` is in preorder and stays in it, each merged fold where
/// its outermost contributor stood.
///
/// Only folds that hide something merge: a fold of fewer than two lines is
/// no region (see `nested_spans`), and two of them covering one line are
/// two byte ranges the reader never sees.
pub(crate) fn merge_spans(folds: &mut Vec<Fold>, lines: &[&str]) {
    let inside = |inner: &SourceRange, outer: &SourceRange| {
        let at = |position: &SourcePosition| (position.line.as_usize(), position.byte_column);
        at(&inner.start) >= at(&outer.start) && at(&inner.end) <= at(&outer.end)
    };
    let mut kept: Vec<Fold> = Vec::with_capacity(folds.len());
    let mut by_span: DftHashMap<(usize, usize), usize> = DftHashMap::default();
    for fold in folds.drain(..) {
        let span = line_span(&fold, lines);
        if span.1 - span.0 < 2 {
            kept.push(fold);
            continue;
        }
        match by_span.entry(span) {
            Entry::Vacant(entry) => {
                entry.insert(kept.len());
                kept.push(fold);
            }
            Entry::Occupied(entry) => {
                let merged = &mut kept[*entry.get()];
                if inside(&fold.range, &merged.range) {
                    merged.range = fold.range;
                    merged.syntax_id = fold.syntax_id;
                    merged.match_kind = fold.match_kind;
                }
                merged.tags.extend(fold.tags);
                merged.tags.sort();
                merged.tags.dedup();
            }
        }
    }
    *folds = kept;
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
