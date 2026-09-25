//! Collapse unchanged stretches far from any change.
//!
//! A line stays visible when it is within `lines` of a changed line on its
//! side, when it is paired with such a line, or when it opens or closes a
//! scope that holds a change on its side. Scopes are the constructs this
//! plugin's queries tag `context:scope` (see `scope_rows`), each covering
//! its signature line through the line that closes it; a file the
//! diff did not parse has none. Every other stretch of unchanged
//! paired lines that is at least `MIN_GAP` lines long collapses, labelled
//! with its line count; a file with no change collapses whole, however
//! short.
//!
//! A stretch is cut at region edges. The part in one list of siblings
//! collapses when it is at least `MIN_GAP` lines long or is the whole
//! stretch; a shorter sliver stays open. When a part spans several
//! siblings, only their enclosing group collapses into one row on each side,
//! provided the two sides' siblings match one for one: leaves sharing an
//! `alignment_id`, or folds sharing a fold state. When they do not, as after
//! a line diff fallback, whose folds are all unpaired, the part is grouped
//! with the other side's part over the same lines, if there is one of two or
//! more siblings; the two groups share a fold state all the same. A fold in
//! it collapses only when every region on the other side in its fold state
//! lies wholly inside the stretch there, so no fold is hidden on one side
//! while it holds changed lines on the other, such as a matched function
//! that moved elsewhere. Both folds of a matched pair are labelled with
//! their line count, and a fold with no partner collapses on its own side.
use diffr_plugin_sdk::{
    anyhow, export, has_tag, is_fold, walk, Draft, FileEntry, Move, Node, Pairing, Plugin, Region,
    Source,
};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

/// Collapsing fewer lines than this saves nothing worth a fold row.
const MIN_GAP: u32 = 3;

/// A fold the queries mark as a scope: a function, a type, a block.
const SCOPE: &str = "context:scope";

pub struct Context {
    options: Options,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    /// Unchanged lines kept on either side of a change.
    lines: u32,
}

/// The first and last line of every scope on `source` that holds a changed
/// line, plus its full header when the query marks the function body.
///
/// A scope region is a whole construct: its first line is the line its
/// signature or header starts on and its last is the line that closes it,
/// since the construct's own node is what the queries tag. Keeping both
/// always shows where a scope opens and where it ends, whatever sits inside.
fn scope_rows(source: &Source, changed: &BTreeSet<u32>) -> BTreeSet<u32> {
    let mut rows = BTreeSet::new();
    walk(&source.regions, &mut |region| {
        if !is_fold(region) || !has_tag(region, SCOPE) {
            return;
        }
        let span = region.range.lines();
        if changed.range(span.clone()).next().is_none() {
            return;
        }
        rows.insert(span.start);
        rows.insert(span.end - 1);
        if let Node::Fold { children } = &region.node {
            if let Some(body) = children.iter().find(|child| has_tag(child, "context:body")) {
                rows.extend(span.start..body.range.start.line);
            }
        }
    });
    rows
}

/// One leaf, as far as context cares.
struct Leaf {
    alignment: u32,
    start: u32,
    end: u32,
    /// Whether a leaf on the other side has the same `alignment_id`.
    paired: bool,
    /// Whether this side paints any byte of it as changed.
    spans: bool,
}

fn leaves(regions: &[Region], other: &BTreeSet<u32>) -> Vec<Leaf> {
    let mut out = Vec::new();
    walk(regions, &mut |region| {
        if let Node::Leaf {
            alignment_id,
            changed,
        } = &region.node
        {
            out.push(Leaf {
                alignment: *alignment_id,
                start: region.range.start.line,
                end: region.range.end.line,
                paired: other.contains(alignment_id),
                spans: !changed.is_empty(),
            });
        }
    });
    out
}

fn leaf_alignments(regions: &[Region]) -> BTreeSet<u32> {
    let mut alignments = BTreeSet::new();
    walk(regions, &mut |region| {
        alignments.extend(region.alignment_id());
    });
    alignments
}

fn unchanged_label(count: u32) -> String {
    match count {
        1 => "1 unchanged line".to_owned(),
        count => format!("{count} unchanged lines"),
    }
}

/// A region a stretch covers in one list of siblings: whole, or the lines
/// `start..end` of a leaf, relative to the leaf's first line.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Member {
    /// `state` is a fold's `fold_state_id`, and `None` for a leaf;
    /// `alignment` is a leaf's `alignment_id`, and `None` for a fold.
    Whole {
        id: u32,
        lines: u32,
        state: Option<u32>,
        alignment: Option<u32>,
    },
    Part {
        id: u32,
        alignment: u32,
        start: u32,
        end: u32,
    },
}

impl Member {
    fn lines(&self) -> u32 {
        match self {
            Self::Whole { lines, .. } => *lines,
            Self::Part { start, end, .. } => end - start,
        }
    }

    /// What must match across sides: the leaf's alignment and which of its
    /// lines, or the fold state of a fold. Every region has its own id on
    /// each side.
    fn key(&self) -> (bool, u32, u32) {
        match self {
            Self::Whole {
                state: Some(state), ..
            } => (true, *state, 0),
            Self::Whole {
                alignment: Some(alignment),
                ..
            } => (false, *alignment, 0),
            Self::Whole { .. } => unreachable!("a region is a fold or a leaf"),
            Self::Part {
                alignment, start, ..
            } => (false, *alignment, *start + 1),
        }
    }
}

/// The parts of the lines `start..end` in each list of siblings, in
/// document order.
fn segments(regions: &[Region], start: u32, end: u32, out: &mut Vec<Vec<Member>>) {
    let mut current = Vec::new();
    for region in regions {
        let (first, last) = (region.range.start.line, region.range.end.line);
        if last <= start || first >= end {
            if !current.is_empty() {
                out.push(std::mem::take(&mut current));
            }
            continue;
        }
        if start <= first && last <= end {
            current.push(Member::Whole {
                id: region.id,
                lines: last - first,
                state: is_fold(region).then_some(region.fold_state_id),
                alignment: region.alignment_id(),
            });
            continue;
        }
        match &region.node {
            Node::Leaf { alignment_id, .. } => current.push(Member::Part {
                id: region.id,
                alignment: *alignment_id,
                start: start.max(first) - first,
                end: end.min(last) - first,
            }),
            Node::Fold { children } => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
                segments(children, start, end, out);
            }
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
}

impl Plugin for Context {
    type Options = Options;

    fn new(options: Options) -> anyhow::Result<Self> {
        Ok(Self { options })
    }

    fn queries(&self) -> anyhow::Result<Vec<diffr_plugin_sdk::QuerySource>> {
        Ok(vec![
            diffr_plugin_sdk::QuerySource {
                language: "rust".into(),
                name: "builtin:context/queries/rust.scm".into(),
                text: include_str!("../queries/rust.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "python".into(),
                name: "builtin:context/queries/python.scm".into(),
                text: include_str!("../queries/python.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "go".into(),
                name: "builtin:context/queries/go.scm".into(),
                text: include_str!("../queries/go.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "javascript".into(),
                name: "builtin:context/queries/javascript.scm".into(),
                text: include_str!("../queries/javascript.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "javascriptjsx".into(),
                name: "builtin:context/queries/javascript.scm".into(),
                text: include_str!("../queries/javascript.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "typescript".into(),
                name: "builtin:context/queries/javascript.scm".into(),
                text: include_str!("../queries/javascript.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "typescripttsx".into(),
                name: "builtin:context/queries/javascript.scm".into(),
                text: include_str!("../queries/javascript.scm").into(),
            },
        ])
    }

    fn classify(&self, _file: &FileEntry) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn mutate(&self, _file: &FileEntry, sides: &Pairing<Source>) -> anyhow::Result<Vec<Move>> {
        let options = &self.options;
        let Pairing::Both { lhs, rhs } = &sides else {
            // A one-sided file is all changed lines.
            return Ok(Vec::new());
        };
        let lhs_leaves = leaves(&lhs.regions, &leaf_alignments(&rhs.regions));
        let rhs_leaves = leaves(&rhs.regions, &leaf_alignments(&lhs.regions));
        let rhs_by_alignment: BTreeMap<u32, &Leaf> = rhs_leaves
            .iter()
            .map(|leaf| (leaf.alignment, leaf))
            .collect();

        // Changed lines on each side, and the lines of rows that change.
        let mut novel = [BTreeSet::new(), BTreeSet::new()];
        let mut seeds = [BTreeSet::new(), BTreeSet::new()];
        // (lhs start, rhs start, length) of every unchanged paired leaf.
        let mut unchanged: Vec<(u32, u32, u32)> = Vec::new();
        for (side, own) in [(0, &lhs_leaves), (1, &rhs_leaves)] {
            for leaf in own.iter() {
                if !leaf.paired || leaf.spans {
                    novel[side].extend(leaf.start..leaf.end);
                    seeds[side].extend(leaf.start..leaf.end);
                }
            }
        }
        for leaf in &lhs_leaves {
            let Some(partner) = rhs_by_alignment
                .get(&leaf.alignment)
                .filter(|_| leaf.paired)
            else {
                continue;
            };
            if leaf.spans || partner.spans {
                seeds[0].extend(leaf.start..leaf.end);
                seeds[1].extend(partner.start..partner.end);
            } else {
                unchanged.push((leaf.start, partner.start, leaf.end - leaf.start));
            }
        }
        let changed = !seeds[0].is_empty() || !seeds[1].is_empty();

        let counts = [lhs, rhs].map(|source| source.text.split_terminator('\n').count() as u32);
        let mut shown = [BTreeSet::new(), BTreeSet::new()];
        for side in 0..2 {
            for &line in &seeds[side] {
                shown[side].extend(
                    line.saturating_sub(options.lines)
                        ..(line + options.lines + 1).min(counts[side]),
                );
            }
        }
        if changed {
            for (side, source) in [lhs, rhs].into_iter().enumerate() {
                // Context adds unchanged rows only; changed rows are shown anyway.
                shown[side].extend(scope_rows(source, &novel[side]));
            }
        }

        // Hidden rows of unchanged paired leaves, as (lhs line, rhs line).
        let mut hidden: Vec<(u32, u32)> = Vec::new();
        for &(lhs_start, rhs_start, len) in &unchanged {
            for offset in 0..len {
                let (l, r) = (lhs_start + offset, rhs_start + offset);
                if !changed || (!shown[0].contains(&l) && !shown[1].contains(&r)) {
                    hidden.push((l, r));
                }
            }
        }
        hidden.sort_unstable();
        let mut stretches: Vec<((u32, u32), (u32, u32))> = Vec::new();
        for (l, r) in hidden {
            match stretches.last_mut() {
                Some(((_, lhs_end), (_, rhs_end))) if *lhs_end == l && *rhs_end == r => {
                    *lhs_end += 1;
                    *rhs_end += 1;
                }
                _ => stretches.push(((l, l + 1), (r, r + 1))),
            }
        }

        let mut lhs_states: BTreeMap<u32, u32> = BTreeMap::new();
        // The lhs regions in each fold state, by id.
        let mut lhs_by_state: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        walk(&lhs.regions, &mut |region| {
            lhs_states.insert(region.id, region.fold_state_id);
            lhs_by_state
                .entry(region.fold_state_id)
                .or_default()
                .push(region.id);
        });
        // The rhs regions in each fold state, by id.
        let mut rhs_by_state: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        walk(&rhs.regions, &mut |region| {
            rhs_by_state
                .entry(region.fold_state_id)
                .or_default()
                .push(region.id);
        });
        // Stretches are shaped in reverse document order, so a cut leaf's id
        // still names the piece that starts where the leaf did.
        let mut draft = Draft::new(sides);
        for ((lhs_start, lhs_end), (rhs_start, rhs_end)) in stretches.into_iter().rev() {
            let total = lhs_end - lhs_start;
            if total < MIN_GAP && changed {
                continue;
            }
            let mut lhs_parts = Vec::new();
            segments(&lhs.regions, lhs_start, lhs_end, &mut lhs_parts);
            let mut rhs_parts = Vec::new();
            segments(&rhs.regions, rhs_start, rhs_end, &mut rhs_parts);
            let key = |parts: &[Vec<Member>]| -> Vec<Vec<(bool, u32, u32)>> {
                parts
                    .iter()
                    .map(|part| part.iter().map(Member::key).collect())
                    .collect()
            };
            let same_shape = key(&lhs_parts) == key(&rhs_parts);
            // The first line and line count of each part, relative to the
            // stretch, which the parts cover in order.
            let spans = |parts: &[Vec<Member>]| -> Vec<(u32, u32)> {
                let mut at = 0;
                parts
                    .iter()
                    .map(|part| {
                        let lines: u32 = part.iter().map(Member::lines).sum();
                        at += lines;
                        (at - lines, lines)
                    })
                    .collect()
            };
            let (lhs_spans, rhs_spans) = (spans(&lhs_parts), spans(&rhs_parts));
            // The leaves a part cuts, by alignment and first line, which a
            // leaf and the leaf paired with it share.
            let cuts = |part: &[Member]| -> Vec<(u32, u32)> {
                part.iter()
                    .filter_map(|member| match member {
                        Member::Part {
                            alignment, start, ..
                        } => Some((*alignment, *start)),
                        Member::Whole { .. } => None,
                    })
                    .collect()
            };
            let whole = |parts: &[Vec<Member>]| -> BTreeSet<u32> {
                parts
                    .iter()
                    .flatten()
                    .filter_map(|member| match member {
                        Member::Whole { id, .. } => Some(*id),
                        Member::Part { .. } => None,
                    })
                    .collect()
            };
            // A fold may collapse when everything its collapse reaches on the
            // other side hides only these unchanged lines.
            let (lhs_whole, rhs_whole) = (whole(&lhs_parts), whole(&rhs_parts));
            let hides_only_this = |id: u32| {
                rhs_by_state
                    .get(&lhs_states[&id])
                    .is_none_or(|ids| ids.iter().all(|id| rhs_whole.contains(id)))
            };
            let rhs_hides_only_this = |state: u32| {
                lhs_by_state
                    .get(&state)
                    .is_none_or(|ids| ids.iter().all(|id| lhs_whole.contains(id)))
            };
            // Line counts of the rhs folds wholly in the stretch, so each
            // side's fold takes its own label.
            let rhs_fold_lines: BTreeMap<u32, u32> = rhs_parts
                .iter()
                .flatten()
                .filter_map(|member| match member {
                    Member::Whole {
                        id,
                        lines,
                        state: Some(_),
                        ..
                    } => Some((*id, *lines)),
                    _ => None,
                })
                .collect();
            // The rhs parts grouped with an lhs part.
            let mut partnered = BTreeSet::new();
            for (index, part) in lhs_parts.iter().enumerate().rev() {
                let lines: u32 = part.iter().map(Member::lines).sum();
                if lines < MIN_GAP && lines != total {
                    continue;
                }
                // The rhs part grouped with this one: the part it matches, or,
                // when the sides' folds do not match, as after a line diff
                // fallback, the part over the same lines. Both groups are
                // made in one join, so they share a fold state.
                let partner = if same_shape {
                    Some(index)
                } else {
                    rhs_spans
                        .iter()
                        .position(|span| *span == lhs_spans[index])
                        .filter(|&other| {
                            rhs_parts[other].len() >= 2
                                && cuts(&rhs_parts[other]) == cuts(part)
                                && rhs_parts[other].iter().all(|member| match member {
                                    Member::Whole {
                                        state: Some(state), ..
                                    } => rhs_hides_only_this(*state),
                                    _ => true,
                                })
                        })
                };
                // One context fold is enough for a group. Leave its members'
                // visibility intact so expanding it reveals code directly.
                let grouped = partner.is_some()
                    && part.len() >= 2
                    && part.iter().all(|member| match member {
                        Member::Whole {
                            id, state: Some(_), ..
                        } => hides_only_this(*id),
                        _ => true,
                    });
                let mut members = Vec::new();
                // The pieces cut from leaves, by their alignment and first
                // line, which the rhs piece of each shares.
                let mut pieces = BTreeMap::new();
                for member in part.iter().rev() {
                    match member {
                        Member::Whole {
                            id, state: Some(_), ..
                        } if !hides_only_this(*id) => {}
                        Member::Whole {
                            id, lines, state, ..
                        } => {
                            if !grouped {
                                draft.collapse(*id, unchanged_label(*lines))?;
                            }
                            members.push(*id);
                            let Some(state) = state else {
                                continue;
                            };
                            for rhs_id in rhs_by_state.get(state).into_iter().flatten() {
                                if let Some(rhs_lines) =
                                    rhs_fold_lines.get(rhs_id).filter(|_| !grouped)
                                {
                                    draft.collapse(*rhs_id, unchanged_label(*rhs_lines))?;
                                }
                            }
                        }
                        Member::Part {
                            id,
                            alignment,
                            start,
                            end,
                        } => {
                            let piece = draft.cut_lines(*id, *start, *end)?;
                            if !grouped {
                                draft.collapse(piece, unchanged_label(end - start))?;
                            }
                            members.push(piece);
                            pieces.insert((*alignment, *start), piece);
                        }
                    }
                }
                // A fold left open breaks the run, so nothing is grouped.
                let Some(partner) = partner.filter(|_| grouped) else {
                    continue;
                };
                partnered.insert(partner);
                members.reverse();
                for member in &rhs_parts[partner] {
                    match member {
                        Member::Whole { id, .. } => members.push(*id),
                        Member::Part {
                            alignment, start, ..
                        } => members.extend(draft.paired_leaf(pieces[&(*alignment, *start)])?),
                    }
                }
                draft.group(members, unchanged_label(lines))?;
            }
            // A fold only the rhs holds collapses on its own: no lhs fold
            // shares its fold state to collapse it with.
            for (index, part) in rhs_parts.iter().enumerate() {
                let lines: u32 = part.iter().map(Member::lines).sum();
                if partnered.contains(&index) || (lines < MIN_GAP && lines != total) {
                    continue;
                }
                for member in part {
                    if let Member::Whole {
                        id,
                        lines,
                        state: Some(state),
                        ..
                    } = member
                    {
                        if !lhs_by_state.contains_key(state) {
                            draft.collapse(*id, unchanged_label(*lines))?;
                        }
                    }
                }
            }
        }
        Ok(draft.into_moves())
    }
}

export!("context", Context);
