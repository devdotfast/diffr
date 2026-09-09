//! Select enclosing syntax context for one change hunk.
use std::collections::BTreeSet;

use super::line_layout::LineSelection;
use crate::parse::syntax::Syntax;

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
