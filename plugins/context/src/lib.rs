//! Collapse unchanged stretches far from any change.
//!
//! A line stays visible when it is within `lines` of a changed line on its
//! side, when it is paired with such a line, or when it opens or closes a
//! scope that holds a change on its side or has a visible boundary. Scopes
//! are the constructs this plugin's queries tag `context:scope` (see
//! `scope_rows`), each covering its signature through its closing line.
//! A `context:body` fold instead excludes both delimiter lines. A file the
//! diff did not parse has none. Every other stretch of unchanged
//! paired lines that is at least `MIN_GAP` lines long collapses, labelled
//! with its line count; a file with no change collapses whole, however
//! short.
//!
//! A stretch is cut at region edges. The part in one list of siblings
//! collapses when it is at least `MIN_GAP` lines long or is the whole
//! stretch; a shorter sliver stays open. When a part spans several
//! siblings, each collapses and a group wraps them in one row on each side,
//! provided the two sides' siblings match one for one: leaves sharing an
//! `alignment_id`, or folds sharing a fold state. A fold in it collapses only when every
//! region on the other side in its fold state lies wholly inside the
//! stretch there, so no fold is hidden on one side while it holds changed
//! lines on the other, such as a matched function that moved elsewhere.
//! Both folds of a matched pair are labelled with their line count.
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

/// A body fold excludes its delimiter lines, unlike a whole-construct scope.
const BODY: &str = "context:body";

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
/// line or has an opening or closing line already visible in the context window.
///
/// A scope region is a whole construct: its first line is the line its
/// signature or header starts on and its last is the line that closes it,
/// since the construct's own node is what the queries tag. Keeping both
/// always shows where a scope opens and where it ends, whatever sits inside.
/// Body folds use the immediately adjacent lines for their delimiters.
fn scope_rows(source: &Source, changed: &BTreeSet<u32>, visible: &BTreeSet<u32>) -> BTreeSet<u32> {
    let mut rows = visible.clone();
    loop {
        let previous = rows.len();
        walk(&source.regions, &mut |region| {
            if !is_fold(region) || !(has_tag(region, SCOPE) || has_tag(region, BODY)) {
                return;
            }
            let span = region.range.lines();
            let (first, last) = if has_tag(region, BODY) {
                (span.start.saturating_sub(1), span.end)
            } else {
                (span.start, span.end - 1)
            };
            if changed.range(span.clone()).next().is_none()
                && !rows.contains(&first)
                && !rows.contains(&last)
            {
                return;
            }
            rows.insert(first);
            rows.insert(last);
        });
        // Shared boundaries connect constructs such as try and catch. Repeat
        // so neither side of such a boundary leaves an unmatched delimiter.
        if rows.len() == previous {
            break;
        }
    }
    rows
}

/// One leaf, as far as context cares.
struct Leaf {
    id: u32,
    alignment: u32,
    start: u32,
    end: u32,
    /// Whether a leaf on the other side has the same `alignment_id`.
    paired: bool,
    /// Whether this side paints any byte of it as changed.
    spans: bool,
    highlights: BTreeSet<u32>,
}

fn leaves(regions: &[Region], other: &BTreeSet<u32>) -> Vec<Leaf> {
    let mut out = Vec::new();
    walk(regions, &mut |region| {
        if let Node::Leaf {
            alignment_id,
            changed,
            search_highlights,
        } = &region.node
        {
            out.push(Leaf {
                id: region.id,
                alignment: *alignment_id,
                start: region.range.start.line,
                end: region.range.end.line,
                paired: other.contains(alignment_id),
                spans: !changed.is_empty(),
                highlights: search_highlights.iter().map(|span| span.line).collect(),
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

// Same processes one unchanged tree. Genuine one-sided diffs retain their
// existing behavior when there are no search highlights.
fn single_side_context(sides: &Pairing<Source>, context: u32) -> anyhow::Result<Vec<Move>> {
    let source = sides.sides()[0];
    if !matches!(sides, Pairing::Same { .. })
        && !source
            .regions
            .iter()
            .any(diffr_plugin_sdk::has_search_highlights)
    {
        return Ok(Vec::new());
    }
    let mut anchors = BTreeSet::new();
    walk(&source.regions, &mut |region| {
        if let Node::Leaf {
            changed,
            search_highlights,
            ..
        } = &region.node
        {
            if !changed.is_empty() {
                anchors.extend(region.range.lines());
            }
            anchors.extend(search_highlights.iter().map(|span| span.line));
        }
    });
    let count = source.text.split_terminator('\n').count() as u32;
    let mut shown = BTreeSet::new();
    for line in &anchors {
        shown.extend(
            line.saturating_sub(context)..line.saturating_add(context).saturating_add(1).min(count),
        );
    }
    shown.extend(scope_rows(source, &anchors, &shown));
    let mut gaps: Vec<(u32, u32)> = Vec::new();
    for line in (0..count).filter(|line| !shown.contains(line)) {
        if let Some((_, end)) = gaps.last_mut().filter(|(_, end)| *end == line) {
            *end += 1;
        } else {
            gaps.push((line, line + 1));
        }
    }
    let mut draft = Draft::new(sides);
    for (start, end) in gaps.into_iter().rev() {
        if end - start < MIN_GAP && !matches!(sides, Pairing::Same { .. }) {
            continue;
        }
        let mut parts = Vec::new();
        segments(&source.regions, start, end, &mut parts);
        for part in parts.into_iter().rev() {
            let length: u32 = part.iter().map(Member::lines).sum();
            if length < MIN_GAP && length != end - start {
                continue;
            }
            let mut members = Vec::new();
            for member in part.into_iter().rev() {
                let (id, lines) = match member {
                    Member::Whole { id, lines, .. } => (id, lines),
                    Member::Part { id, start, end, .. } => {
                        (draft.cut_lines(id, start, end)?, end - start)
                    }
                };
                draft.collapse(id, unchanged_label(lines))?;
                members.push(id);
            }
            members.reverse();
            if members.len() > 1 {
                draft.group(members, unchanged_label(length))?;
            }
        }
    }
    Ok(draft.into_moves())
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
            return single_side_context(sides, options.lines);
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
        // Search matches seed their actual rows, not the whole containing leaf.
        // Keep unchanged leaves eligible for cutting around those rows.
        for (side, own, other) in [(0, &lhs_leaves, &rhs_leaves), (1, &rhs_leaves, &lhs_leaves)] {
            for leaf in own {
                novel[side].extend(&leaf.highlights);
                seeds[side].extend(&leaf.highlights);
                if let Some(partner) = other.iter().find(|other| other.alignment == leaf.alignment)
                {
                    seeds[1 - side].extend(
                        leaf.highlights
                            .iter()
                            .map(|line| partner.start + line - leaf.start),
                    );
                }
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
                let boundaries = scope_rows(source, &novel[side], &shown[side]);
                shown[side].extend(boundaries);
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
        walk(&lhs.regions, &mut |region| {
            lhs_states.insert(region.id, region.fold_state_id);
        });
        // The rhs regions in each fold state, by id.
        let mut rhs_by_state: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
        walk(&rhs.regions, &mut |region| {
            rhs_by_state
                .entry(region.fold_state_id)
                .or_default()
                .push(region.id);
        });
        // The rhs leaf paired with each lhs leaf, by id.
        let rhs_leaf_ids: BTreeMap<u32, u32> = lhs_leaves
            .iter()
            .filter_map(|leaf| Some((leaf.id, rhs_by_alignment.get(&leaf.alignment)?.id)))
            .collect();
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
            // A fold may collapse when everything its collapse reaches on the
            // rhs hides only these unchanged lines.
            let rhs_whole: BTreeSet<u32> = rhs_parts
                .iter()
                .flatten()
                .filter_map(|member| match member {
                    Member::Whole { id, .. } => Some(*id),
                    Member::Part { .. } => None,
                })
                .collect();
            let hides_only_this = |id: u32| {
                rhs_by_state
                    .get(&lhs_states[&id])
                    .is_none_or(|ids| ids.iter().all(|id| rhs_whole.contains(id)))
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
            for (index, part) in lhs_parts.iter().enumerate().rev() {
                let lines: u32 = part.iter().map(Member::lines).sum();
                if lines < MIN_GAP && lines != total {
                    continue;
                }
                let mut members = Vec::new();
                // The rhs leaves and pieces the lhs members pair with.
                let mut rhs_members = Vec::new();
                for member in part.iter().rev() {
                    match member {
                        Member::Whole {
                            id, state: Some(_), ..
                        } if !hides_only_this(*id) => {}
                        Member::Whole {
                            id, lines, state, ..
                        } => {
                            draft.collapse(*id, unchanged_label(*lines))?;
                            members.push(*id);
                            let Some(state) = state else {
                                rhs_members.extend(rhs_leaf_ids.get(id));
                                continue;
                            };
                            for rhs_id in rhs_by_state.get(state).into_iter().flatten() {
                                if let Some(rhs_lines) = rhs_fold_lines.get(rhs_id) {
                                    draft.collapse(*rhs_id, unchanged_label(*rhs_lines))?;
                                }
                            }
                        }
                        Member::Part { id, start, end, .. } => {
                            let piece = draft.cut_lines(*id, *start, *end)?;
                            draft.collapse(piece, unchanged_label(end - start))?;
                            members.push(piece);
                            rhs_members.extend(draft.paired_leaf(piece)?);
                        }
                    }
                }
                // A fold left open breaks the run, so nothing is grouped.
                if same_shape && members.len() == part.len() && members.len() >= 2 {
                    members.reverse();
                    // The rhs leaves and pieces, then the rhs folds.
                    members.extend(rhs_members);
                    members.extend(rhs_parts[index].iter().filter_map(|member| match member {
                        Member::Whole {
                            id, state: Some(_), ..
                        } => Some(*id),
                        _ => None,
                    }));
                    draft.group(members, unchanged_label(lines))?;
                }
            }
        }
        Ok(draft.into_moves())
    }
}

export!("context", Context);
