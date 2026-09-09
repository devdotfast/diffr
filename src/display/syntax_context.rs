//! Select enclosing syntax context for one change hunk.
use std::collections::BTreeSet;

use super::hunks::{ContextRange, Hunk};
use super::line_layout::{self as layout, LineSelection};
use crate::lines::SourceRange;
use crate::parse::syntax::{MatchedPos, Syntax};

/// Syntax candidates are discovered once, before hunks are constructed.
#[derive(Clone, Debug, Default)]
pub(crate) struct SyntaxAnnotations {
    lhs_candidates: Vec<ContextCandidate>,
    rhs_candidates: Vec<ContextCandidate>,
}

#[derive(Clone, Debug)]
pub(crate) struct ContextCandidate {
    pub(crate) contains: std::ops::RangeInclusive<usize>,
    pub(crate) rows: BTreeSet<usize>,
}

impl SyntaxAnnotations {
    pub(crate) fn collect((lhs, rhs): (&[&Syntax<'_>], &[&Syntax<'_>])) -> Self {
        Self {
            lhs_candidates: candidates(lhs),
            rhs_candidates: candidates(rhs),
        }
    }

    pub(crate) fn context_for_changes(
        &self,
        lhs: &crate::hash::DftHashSet<line_numbers::LineNumber>,
        rhs: &crate::hash::DftHashSet<line_numbers::LineNumber>,
    ) -> LineSelection {
        LineSelection {
            lhs: select_candidates(&self.lhs_candidates, lhs),
            rhs: select_candidates(&self.rhs_candidates, rhs),
        }
    }
}

fn candidates(roots: &[&Syntax<'_>]) -> Vec<ContextCandidate> {
    let mut contexts = Vec::new();
    let mut occupied = BTreeSet::new();
    let mut pending = roots.to_vec();
    while let Some(node) = pending.pop() {
        contexts.extend(node.info().context.borrow().iter().cloned());
        match node {
            Syntax::Atom { position, .. } => {
                occupied.extend(position.iter().map(|span| span.line.as_usize()))
            }
            Syntax::List {
                open_position,
                close_position,
                children,
                ..
            } => {
                occupied.extend(
                    open_position
                        .iter()
                        .chain(close_position)
                        .filter(|span| span.start_col < span.end_col)
                        .map(|span| span.line.as_usize()),
                );
                pending.extend(children);
            }
        }
    }
    contexts
        .into_iter()
        .map(|context| {
            let mut rows: BTreeSet<_> = occupied.range(context.header).copied().collect();
            rows.extend(context.closing);
            ContextCandidate {
                contains: context.contains,
                rows,
            }
        })
        .collect()
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
        let selected = annotations.context_for_changes(&hunk.novel_lhs, &hunk.novel_rhs);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{guess_language::Language, tree_sitter_parser as parser};
    use typed_arena::Arena;

    #[test]
    fn context_survives_without_source_or_tree() {
        let arena = Arena::new();
        let syntax = {
            let source = String::from(
                "def run(\n    value,\n):\n    return {\n        'key': value,\n    }\n",
            );
            parser::parse(
                &arena,
                &source,
                parser::from_language(Language::Python),
                false,
            )
        };
        let selected = select_candidates(
            &candidates(&syntax),
            &std::iter::once(line_numbers::LineNumber::from(4)).collect(),
        );
        assert_eq!(selected, BTreeSet::from([0, 1, 2, 3, 5]));
    }

    #[test]
    fn atomic_tail_retains_context_after_wrapper_flattening() {
        let arena = Arena::new();
        let syntax = parser::parse(
            &arena,
            "fn run() -> i32 {\n    answer\n}\n",
            parser::from_language(Language::Rust),
            false,
        );
        let mut pending = syntax;
        let mut found_tail = false;
        while let Some(node) = pending.pop() {
            match node {
                Syntax::Atom { content, .. } if content == "answer" => {
                    assert!(node
                        .info()
                        .context
                        .borrow()
                        .iter()
                        .any(|context| context.contains == (1..=1)));
                    found_tail = true;
                }
                Syntax::List { children, .. } => pending.extend(children),
                _ => {}
            }
        }
        assert!(found_tail);
    }
}
