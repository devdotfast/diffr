//! Select enclosing syntax context for one change hunk.
use std::collections::BTreeSet;
use tree_sitter::{Node, Tree};

use super::hunks::{ContextRange, Hunk};
use super::line_layout::{self as layout, LineSelection};
use crate::lines::SourceRange;
use crate::parse::syntax::MatchedPos;

/// Syntax candidates are discovered once, before hunks are constructed.
#[derive(Default)]
pub(crate) struct SyntaxAnnotations {
    lhs_candidates: Vec<ContextCandidate>,
    rhs_candidates: Vec<ContextCandidate>,
}

struct ContextCandidate {
    contains: std::ops::RangeInclusive<usize>,
    rows: BTreeSet<usize>,
}

impl SyntaxAnnotations {
    pub(crate) fn collect(
        (lhs_tree, rhs_tree): (&Tree, &Tree),
        (lhs_src, rhs_src): (&str, &str),
    ) -> Self {
        Self {
            lhs_candidates: candidates(lhs_tree, lhs_src),
            rhs_candidates: candidates(rhs_tree, rhs_src),
        }
    }

    fn context_for_hunk(&self, hunk: &crate::display::hunks::Hunk) -> LineSelection {
        LineSelection {
            lhs: select_candidates(&self.lhs_candidates, &hunk.novel_lhs),
            rhs: select_candidates(&self.rhs_candidates, &hunk.novel_rhs),
        }
    }
}

fn candidates(tree: &Tree, source: &str) -> Vec<ContextCandidate> {
    let lines: Vec<_> = source.split('\n').collect();
    let mut candidates = Vec::new();
    collect_candidates(tree.root_node(), &lines, &mut candidates);
    candidates
}

fn select_candidates(
    candidates: &[ContextCandidate],
    changed: &crate::hash::DftHashSet<line_numbers::LineNumber>,
) -> BTreeSet<usize> {
    let changed: BTreeSet<_> = changed.iter().map(|line| line.as_usize()).collect();
    candidates
        .iter()
        .filter(|candidate| changed.range(candidate.contains.clone()).next().is_some())
        .flat_map(|candidate| candidate.rows.iter().copied())
        .collect()
}

fn is_function(kind: &str) -> bool {
    matches!(
        kind,
        "function_definition"
            | "function_declaration"
            | "method_definition"
            | "method_declaration"
            | "function_item"
            | "arrow_function"
    )
}
fn is_scope(kind: &str) -> bool {
    is_function(kind)
        || matches!(
            kind,
            "class_definition" | "class_declaration" | "impl_item" | "mod_item" | "struct_item"
        )
}

fn add_lines(out: &mut BTreeSet<usize>, start: usize, end: usize, src: &[&str]) {
    out.extend((start..=end).filter(|r| *r < src.len() && !src[*r].trim().is_empty()));
}

fn header_end(node: Node<'_>, body: Node<'_>) -> usize {
    if body.kind() == "block"
        && node.kind().ends_with("definition")
        && body.start_position().row > node.start_position().row
    {
        body.start_position().row - 1
    } else {
        body.start_position().row
    }
}

fn scope_body(node: Node<'_>) -> Option<Node<'_>> {
    if !is_scope(node.kind()) {
        return None;
    }
    node.child_by_field_name("body")
}

fn closing_brace_line(body: Node<'_>, source: &[&str]) -> Option<usize> {
    let row = body.end_position().row;
    source
        .get(row)
        .filter(|line| line.trim_start().starts_with('}'))
        .map(|_| row)
}

fn add_boundaries(node: Node<'_>, out: &mut BTreeSet<usize>, src: &[&str]) {
    add_lines(
        out,
        node.start_position().row,
        node.start_position().row,
        src,
    );
    add_lines(out, node.end_position().row, node.end_position().row, src);
}

fn candidate(node: Node<'_>, rows: BTreeSet<usize>) -> ContextCandidate {
    ContextCandidate {
        contains: node.start_position().row..=node.end_position().row,
        rows,
    }
}

fn collect_candidates(node: Node<'_>, src: &[&str], out: &mut Vec<ContextCandidate>) {
    if let Some(body) = scope_body(node) {
        let mut rows = BTreeSet::new();
        add_lines(
            &mut rows,
            node.start_position().row,
            header_end(node, body),
            src,
        );
        rows.extend(closing_brace_line(body, src));
        out.push(candidate(node, rows));
        collect_tail(node, body, src, out);
    }
    if matches!(
        node.kind(),
        "for_statement"
            | "for_expression"
            | "loop_expression"
            | "expression_switch_statement"
            | "switch_statement"
            | "match_expression"
            | "return_statement"
            | "return_expression"
    ) {
        let mut rows = BTreeSet::new();
        add_boundaries(node, &mut rows, src);
        out.push(candidate(node, rows));
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_candidates(child, src, out);
    }
}

fn collect_tail(node: Node<'_>, body: Node<'_>, src: &[&str], out: &mut Vec<ContextCandidate>) {
    if node.kind() != "function_item" {
        return;
    }
    let mut cursor = body.walk();
    let Some(tail) = body.named_children(&mut cursor).last() else {
        return;
    };
    if matches!(
        tail.kind(),
        "let_declaration" | "expression_statement" | "line_comment" | "block_comment"
    ) {
        return;
    }
    let mut rows = BTreeSet::new();
    add_boundaries(tail, &mut rows, src);
    out.push(candidate(tail, rows));
}

pub(crate) fn add_hunk_context(
    hunks: &mut [Hunk],
    (lhs_src, rhs_src): (&str, &str),
    (lhs_positions, rhs_positions): (&[MatchedPos], &[MatchedPos]),
    annotations: &SyntaxAnnotations,
) {
    let rows = layout::aligned_rows((lhs_src, rhs_src), (lhs_positions, rhs_positions));
    let lhs_lines: Vec<_> = lhs_src.split('\n').collect();
    let rhs_lines: Vec<_> = rhs_src.split('\n').collect();
    let lhs_novel = layout::novel_lines(lhs_positions);
    let rhs_novel = layout::novel_lines(rhs_positions);
    let mut lhs_index = vec![None; lhs_lines.len()];
    let mut rhs_index = vec![None; rhs_lines.len()];
    let mut shared_rows = Vec::new();
    for (lhs, rhs) in rows {
        let (Some(lhs), Some(rhs)) = (lhs, rhs) else {
            continue;
        };
        if lhs_novel.contains(&lhs) || rhs_novel.contains(&rhs) {
            continue;
        }
        let index = shared_rows.len();
        shared_rows.push((lhs, rhs));
        lhs_index[lhs] = Some(index);
        rhs_index[rhs] = Some(index);
    }
    for hunk in hunks {
        let selected = annotations.context_for_hunk(hunk);
        let indexes: std::collections::BTreeSet<_> = selected
            .lhs
            .iter()
            .filter_map(|&line| lhs_index[line])
            .chain(selected.rhs.iter().filter_map(|&line| rhs_index[line]))
            .collect();
        hunk.context = indexes
            .into_iter()
            .map(|index| {
                let (lhs, rhs) = shared_rows[index];
                ContextRange {
                    lhs: SourceRange::line(lhs_lines[lhs], lhs),
                    rhs: SourceRange::line(rhs_lines[rhs], rhs),
                }
            })
            .collect();
    }
}

/// Context is accumulated as paired rows during the single hunk merge pass.
/// Compact the union once, after merging, rather than rebuilding it at each step.
pub(crate) fn compact_hunk_context(hunks: &mut [Hunk]) {
    for hunk in hunks {
        let mut context = std::mem::take(&mut hunk.context);
        context.sort_by_key(|r| (r.lhs.start.line, r.rhs.start.line));
        context.dedup_by_key(|r| (r.lhs.start.line, r.rhs.start.line));
        for region in context {
            let Some(previous) = hunk.context.last_mut() else {
                hunk.context.push(region);
                continue;
            };
            if previous.lhs.rows().end() + 1 == region.lhs.start.line.as_usize()
                && previous.rhs.rows().end() + 1 == region.rhs.start.line.as_usize()
            {
                previous.lhs.end = region.lhs.end;
                previous.rhs.end = region.rhs.end;
                continue;
            }
            hunk.context.push(region);
        }
    }
}
