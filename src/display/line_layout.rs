//! Line alignment, ordinary context padding, and indentation classification.
use super::hunks::Hunk;
use crate::display::context::all_matched_lines_filled;
use crate::parse::syntax::MatchedPos;
use crate::summary::{DiffResult, FileContent};
use std::collections::BTreeSet;

#[derive(Default)]
pub(crate) struct LineSelection {
    pub(crate) lhs: BTreeSet<usize>,
    pub(crate) rhs: BTreeSet<usize>,
}

impl LineSelection {
    pub(crate) fn from_hunks(hunks: &[Hunk]) -> Self {
        let mut selection = Self::default();
        for (lhs, rhs) in hunks.iter().flat_map(|hunk| &hunk.lines) {
            selection.lhs.extend(lhs.map(|line| line.as_usize()));
            selection.rhs.extend(rhs.map(|line| line.as_usize()));
        }
        selection
    }
}

pub(crate) type Row = (Option<usize>, Option<usize>);

pub(crate) fn sources(diff: &DiffResult) -> (&str, &str) {
    match (&diff.lhs_src, &diff.rhs_src) {
        (FileContent::Text(lhs), FileContent::Text(rhs)) => (lhs, rhs),
        _ => unreachable!("review entry point accepts text only"),
    }
}

pub(crate) fn novel_lines(positions: &[MatchedPos]) -> BTreeSet<usize> {
    positions
        .iter()
        .filter(|position| position.kind.is_novel())
        .map(|position| position.pos.line.as_usize())
        .collect()
}

pub(crate) fn aligned_rows(
    (lhs_src, rhs_src): (&str, &str),
    (lhs_positions, rhs_positions): (&[MatchedPos], &[MatchedPos]),
) -> Vec<Row> {
    let lhs_lines: Vec<_> = lhs_src.split_terminator('\n').collect();
    let rhs_lines: Vec<_> = rhs_src.split_terminator('\n').collect();
    let mut lhs_seen = BTreeSet::new();
    let mut rhs_seen = BTreeSet::new();
    all_matched_lines_filled(lhs_positions, rhs_positions, &lhs_lines, &rhs_lines)
        .into_iter()
        .filter_map(|(lhs, rhs)| {
            let lhs = lhs
                .map(|line| line.as_usize())
                .filter(|&line| line < lhs_lines.len() && lhs_seen.insert(line));
            let rhs = rhs
                .map(|line| line.as_usize())
                .filter(|&line| line < rhs_lines.len() && rhs_seen.insert(line));
            if lhs.is_none() && rhs.is_none() {
                return None;
            }
            Some((lhs, rhs))
        })
        .collect()
}

/// Only treat indentation as formatting when the matcher confirms a pairing
/// and neither line contains novel syntax. Never normalize string contents.
pub(crate) fn reindented_pairs(diff: &DiffResult) -> BTreeSet<(usize, usize)> {
    use crate::parse::syntax::MatchKind;
    let (lhs_src, rhs_src) = sources(diff);
    let lhs_lines: Vec<_> = lhs_src.split_terminator('\n').collect();
    let rhs_lines: Vec<_> = rhs_src.split_terminator('\n').collect();
    let lhs_novel = novel_lines(&diff.lhs_positions);
    let rhs_novel = novel_lines(&diff.rhs_positions);
    let mut pairs = BTreeSet::new();
    for position in &diff.lhs_positions {
        let MatchKind::UnchangedToken { opposite_pos, .. } = &position.kind else {
            continue;
        };
        let lhs = position.pos.line.as_usize();
        if lhs_novel.contains(&lhs) || lhs >= lhs_lines.len() {
            continue;
        }
        for opposite in opposite_pos {
            let rhs = opposite.line.as_usize();
            if rhs_novel.contains(&rhs) || rhs >= rhs_lines.len() {
                continue;
            }
            let lhs_text = lhs_lines[lhs];
            let rhs_text = rhs_lines[rhs];
            if lhs_text == rhs_text || lhs_text.trim_start() != rhs_text.trim_start() {
                continue;
            }
            pairs.insert((lhs, rhs));
        }
    }
    pairs
}
