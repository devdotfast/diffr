//! Complete the selected rows before publishing a DiffResult.
use super::hunks::Hunk;
use super::line_layout;
use super::syntax_context::SyntaxAnnotations;
use crate::parse::syntax::MatchedPos;
use line_numbers::LineNumber;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn prepare(
    hunks: &[Hunk],
    sources: (&str, &str),
    positions: (&[MatchedPos], &[MatchedPos]),
    annotations: &SyntaxAnnotations,
    padding: usize,
) -> (Vec<Hunk>, Vec<line_layout::Row>) {
    let rows = line_layout::aligned_rows(sources, positions);
    let lhs_novel = line_layout::novel_lines(positions.0);
    let rhs_novel = line_layout::novel_lines(positions.1);
    let mut lhs_index = BTreeMap::new();
    let mut rhs_index = BTreeMap::new();
    for (index, (lhs, rhs)) in rows.iter().enumerate() {
        lhs_index.extend(lhs.map(|line| (line, index)));
        rhs_index.extend(rhs.map(|line| (line, index)));
    }

    let mut selections = Vec::new();
    for hunk in hunks {
        let mut seeds = BTreeSet::new();
        for (lhs, rhs) in &hunk.lines {
            seeds.extend(
                lhs.and_then(|line| lhs_index.get(&line.as_usize()))
                    .copied(),
            );
            seeds.extend(
                rhs.and_then(|line| rhs_index.get(&line.as_usize()))
                    .copied(),
            );
        }
        let context = annotations.context_for_changes(&hunk.novel_lhs, &hunk.novel_rhs);
        let context_indexes = context
            .lhs
            .iter()
            .filter_map(|line| lhs_index.get(line))
            .chain(context.rhs.iter().filter_map(|line| rhs_index.get(line)));
        for &index in context_indexes {
            let (Some(lhs), Some(rhs)) = rows[index] else {
                continue;
            };
            if !lhs_novel.contains(&lhs) && !rhs_novel.contains(&rhs) {
                seeds.insert(index);
            }
        }
        let mut selected = BTreeSet::new();
        for index in seeds {
            selected.extend(index.saturating_sub(padding)..(index + padding + 1).min(rows.len()));
        }
        selections.push(selected);
    }

    selections.sort_by_key(|selection| selection.first().copied());
    let mut groups: Vec<BTreeSet<usize>> = Vec::new();
    for selection in selections {
        if let Some(previous) = groups.last_mut() {
            // Touching or interleaving windows share a hunk, without exposing gaps.
            if selection
                .first()
                .zip(previous.last())
                .is_some_and(|(first, last)| *first <= last.saturating_add(1))
            {
                previous.extend(selection);
                continue;
            }
        }
        groups.push(selection);
    }
    let hunks = groups
        .into_iter()
        .map(|selected| {
            let lines: Vec<_> = selected
                .into_iter()
                .map(|index| {
                    let (lhs, rhs) = rows[index];
                    (
                        lhs.map(|line| LineNumber(line as u32)),
                        rhs.map(|line| LineNumber(line as u32)),
                    )
                })
                .collect();
            let novel_lhs = lines
                .iter()
                .filter_map(|(lhs, _)| *lhs)
                .filter(|line| lhs_novel.contains(&line.as_usize()))
                .collect();
            let novel_rhs = lines
                .iter()
                .filter_map(|(_, rhs)| *rhs)
                .filter(|line| rhs_novel.contains(&line.as_usize()))
                .collect();
            Hunk {
                lines,
                novel_lhs,
                novel_rhs,
            }
        })
        .collect();
    (hunks, rows)
}
