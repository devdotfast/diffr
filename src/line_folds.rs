//! Fold pairing and row alignment for files that fell back to a line diff.
//!
//! A structural diff pairs folds through the syntax matcher (see
//! `parse::folds::pair_matched`). A line diff has no matcher, so folds pair
//! through the line alignment instead: first by header line, then through
//! the lines they hold. The same alignment, with changed blocks re-paired
//! from whichever end reads alike, is what the projection lays rows out on.

use std::collections::{BTreeMap, BTreeSet};

use crate::display::line_layout::{aligned_rows, novel_lines, Row};
use crate::hash::DftHashMap;
use crate::lines::SourceRange;
use crate::parse::folds::{Fold, FoldMatch};
use crate::parse::syntax::MatchedPos;

/// The row alignment of a line-diff fallback: the line diff's rows, with
/// each changed block re-paired by `align_changed_blocks`.
pub(crate) fn fallback_rows(
    (lhs_src, rhs_src): (&str, &str),
    (lhs_positions, rhs_positions): (&[MatchedPos], &[MatchedPos]),
) -> Vec<Row> {
    let lhs_lines: Vec<&str> = lhs_src.split_terminator('\n').collect();
    let rhs_lines: Vec<&str> = rhs_src.split_terminator('\n').collect();
    let rows = aligned_rows((lhs_src, rhs_src), (lhs_positions, rhs_positions));
    align_changed_blocks(
        &rows,
        &novel_lines(lhs_positions),
        &novel_lines(rhs_positions),
        (&lhs_lines, &rhs_lines),
    )
}

/// Pair the folds of a line-diff fallback, as `(lhs index, rhs index)`.
///
/// Rule 1: two folds pair when the alignment pairs their header lines and
/// their tags agree. A changed signature is still a paired row, so a
/// function whose header was edited keeps its counterpart. Folds sharing a
/// header line on one side pair in order of span length.
///
/// Rule 2, for a lhs fold rule 1 left unpaired: the free rhs fold with
/// equal tags that holds the most aligned counterparts of the lhs fold's
/// lines (ties by the nearest header in row order), provided the pair does
/// not cross an existing pair. This keeps a function whose signature was
/// reflowed onto more lines paired with itself.
pub(crate) fn pair(
    rows: &[Row],
    (lhs_src, rhs_src): (&str, &str),
    (lhs_folds, rhs_folds): (&mut [Fold], &mut [Fold]),
) {
    let lhs_spans = spans(lhs_folds, lhs_src.split_terminator('\n').count());
    let rhs_spans = spans(rhs_folds, rhs_src.split_terminator('\n').count());
    let mut lhs_pair: Vec<Option<usize>> = vec![None; lhs_folds.len()];
    let mut rhs_pair: Vec<Option<usize>> = vec![None; rhs_folds.len()];

    let aligned: DftHashMap<usize, usize> = rows
        .iter()
        .filter_map(|&(lhs, rhs)| Some((lhs?, rhs?)))
        .collect();
    let by_header = |spans: &[Option<(usize, usize)>]| {
        let mut map: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for (index, span) in spans.iter().enumerate() {
            if let Some((start, _)) = span {
                map.entry(*start).or_default().push(index);
            }
        }
        for candidates in map.values_mut() {
            candidates.sort_by_key(|&index| {
                let (start, end) = spans[index].expect("indexed spans exist");
                std::cmp::Reverse(end - start)
            });
        }
        map
    };
    let rhs_by_header = by_header(&rhs_spans);
    for (header, own) in by_header(&lhs_spans) {
        let Some(other) = aligned
            .get(&header)
            .and_then(|line| rhs_by_header.get(line))
        else {
            continue;
        };
        for lhs_index in own {
            let tags = &lhs_folds[lhs_index].tags;
            let Some(&rhs_index) = other
                .iter()
                .find(|&&index| rhs_pair[index].is_none() && rhs_folds[index].tags == *tags)
            else {
                continue;
            };
            lhs_pair[lhs_index] = Some(rhs_index);
            rhs_pair[rhs_index] = Some(lhs_index);
        }
    }

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
    let header_row = |rows_of: &DftHashMap<usize, usize>, span: Option<(usize, usize)>| {
        span.and_then(|(start, _)| rows_of.get(&start).copied())
    };
    let mut pairs: Vec<(usize, usize)> = lhs_pair
        .iter()
        .enumerate()
        .filter_map(|(lhs_index, rhs_index)| {
            Some((
                header_row(&row_of_lhs, lhs_spans[lhs_index])?,
                header_row(&row_of_rhs, rhs_spans[(*rhs_index)?])?,
            ))
        })
        .collect();
    for lhs_index in 0..lhs_folds.len() {
        let Some((lhs_start, lhs_end)) = lhs_spans[lhs_index] else {
            continue;
        };
        if lhs_pair[lhs_index].is_some() {
            continue;
        }
        let Some(lhs_row) = row_of_lhs.get(&lhs_start).copied() else {
            continue;
        };
        let mut score: DftHashMap<usize, usize> = DftHashMap::default();
        for &(lhs, rhs) in rows {
            let (Some(l), Some(r)) = (lhs, rhs) else {
                continue;
            };
            if l < lhs_start || l >= lhs_end {
                continue;
            }
            for (rhs_index, span) in rhs_spans.iter().enumerate() {
                let Some((rhs_start, rhs_end)) = span else {
                    continue;
                };
                if rhs_pair[rhs_index].is_none()
                    && rhs_folds[rhs_index].tags == lhs_folds[lhs_index].tags
                    && *rhs_start <= r
                    && r < *rhs_end
                {
                    *score.entry(rhs_index).or_default() += 1;
                }
            }
        }
        let mut candidates: Vec<(usize, usize, usize)> = score
            .into_iter()
            .filter_map(|(rhs_index, lines)| {
                Some((
                    rhs_index,
                    lines,
                    header_row(&row_of_rhs, rhs_spans[rhs_index])?,
                ))
            })
            .collect();
        candidates.sort_by_key(|&(rhs_index, lines, rhs_row)| {
            (
                std::cmp::Reverse(lines),
                rhs_row.abs_diff(lhs_row),
                rhs_index,
            )
        });
        let Some(&(rhs_index, _, rhs_row)) = candidates
            .iter()
            .find(|&&(_, _, rhs_row)| !pairs.iter().any(|&(a, b)| (a < lhs_row) != (b < rhs_row)))
        else {
            continue;
        };
        lhs_pair[lhs_index] = Some(rhs_index);
        rhs_pair[rhs_index] = Some(lhs_index);
        pairs.push((lhs_row, rhs_row));
    }

    for (lhs_index, rhs_index) in lhs_pair.into_iter().enumerate() {
        let Some(rhs_index) = rhs_index else {
            continue;
        };
        let (lhs_range, rhs_range) = (lhs_folds[lhs_index].range, rhs_folds[rhs_index].range);
        lhs_folds[lhs_index].match_kind = FoldMatch::Unchanged {
            opposite: rhs_range,
        };
        rhs_folds[rhs_index].match_kind = FoldMatch::Unchanged {
            opposite: lhs_range,
        };
    }
}

/// The whole lines each fold covers, or `None` for a fold that hides
/// nothing (a single line).
fn spans(folds: &[Fold], line_count: usize) -> Vec<Option<(usize, usize)>> {
    folds
        .iter()
        .map(|fold| {
            let (start, end) = line_span(&fold.range, line_count);
            (end - start >= 2).then_some((start, end))
        })
        .collect()
}

/// Half-open whole lines a range touches, clamped to the file.
pub(crate) fn line_span(range: &SourceRange, line_count: usize) -> (usize, usize) {
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

/// Re-pair the lines of each changed block, a maximal stretch of rows
/// between unchanged rows, when its two sides differ in length. A line diff
/// pairs a block's first lines with each other; when the block's last lines
/// read more alike, pair those instead, so a changed signature sits next to
/// the old one right above a body that still aligns. Blocks that are
/// one-sided, equal in length, or read no better from the bottom keep their
/// rows.
pub(crate) fn align_changed_blocks(
    rows: &[Row],
    lhs_novel: &BTreeSet<usize>,
    rhs_novel: &BTreeSet<usize>,
    (lhs_lines, rhs_lines): (&[&str], &[&str]),
) -> Vec<Row> {
    let unchanged = |&(lhs, rhs): &Row| matches!((lhs, rhs), (Some(l), Some(r)) if !lhs_novel.contains(&l) && !rhs_novel.contains(&r));
    let mut out = Vec::with_capacity(rows.len());
    let mut at = 0;
    while at < rows.len() {
        if unchanged(&rows[at]) {
            out.push(rows[at]);
            at += 1;
            continue;
        }
        let end = rows[at..]
            .iter()
            .position(unchanged)
            .map_or(rows.len(), |offset| at + offset);
        let block = &rows[at..end];
        let lhs: Vec<usize> = block.iter().filter_map(|&(l, _)| l).collect();
        let rhs: Vec<usize> = block.iter().filter_map(|&(_, r)| r).collect();
        let shared = lhs.len().min(rhs.len());
        let similarity = |lhs_from: usize, rhs_from: usize| -> f64 {
            (0..shared)
                .map(|offset| {
                    line_similarity(
                        lhs_lines[lhs[lhs_from + offset]],
                        rhs_lines[rhs[rhs_from + offset]],
                    )
                })
                .sum()
        };
        let bottom_reads_better = shared > 0
            && lhs.len() != rhs.len()
            && similarity(lhs.len() - shared, rhs.len() - shared) > similarity(0, 0);
        if bottom_reads_better {
            out.extend(lhs[..lhs.len() - shared].iter().map(|&l| (Some(l), None)));
            out.extend(rhs[..rhs.len() - shared].iter().map(|&r| (None, Some(r))));
            out.extend(
                lhs[lhs.len() - shared..]
                    .iter()
                    .zip(&rhs[rhs.len() - shared..])
                    .map(|(&l, &r)| (Some(l), Some(r))),
            );
        } else {
            out.extend_from_slice(block);
        }
        at = end;
    }
    out
}

/// How alike two lines read: the Dice coefficient of their identifier and
/// number tokens, 0 when neither has any.
fn line_similarity(lhs: &str, rhs: &str) -> f64 {
    let tokens = |line: &str| -> BTreeSet<String> {
        line.split(|c: char| !(c.is_alphanumeric() || c == '_'))
            .filter(|token| !token.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let (lhs, rhs) = (tokens(lhs), tokens(rhs));
    if lhs.is_empty() && rhs.is_empty() {
        return 0.0;
    }
    2.0 * lhs.intersection(&rhs).count() as f64 / (lhs.len() + rhs.len()) as f64
}
