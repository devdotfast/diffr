//! The last fold mutation: tidy what the earlier ones collapsed.
//!
//! Adjacent context gaps become one gap, and a run of adjacent sibling
//! folds that all start collapsed is wrapped in one new `group` fold so the
//! reader sees one row instead of a stack of them. Expanding the group
//! reveals each child's own collapsed row. Both steps keep ids paired: a
//! merge or a group happens on one side only when the other side either
//! does the same to the same ids or holds none of them.
use super::{ids, is_fold, one_sided, walk, FoldMutation};
use crate::hash::{DftHashMap, DftHashSet};
use crate::protocol::{
    FileChange, Node, Pairing, Problem, Region, Source, SourceRange, Visibility,
};

pub(crate) struct GroupCollapsed;

impl FoldMutation for GroupCollapsed {
    fn apply(&self, _file: &FileChange, sides: &mut Pairing<Source>) -> Result<(), Problem> {
        let mut next_id = next_id(sides);
        merge_gaps(sides);
        group_folds(sides, &mut next_id);
        Ok(())
    }
}

pub(super) fn next_id(sides: &Pairing<Source>) -> u32 {
    let mut max = None;
    for source in [sides.lhs(), sides.rhs()].into_iter().flatten() {
        walk(&source.regions, &mut |region| {
            max = max.max(Some(region.alignment_id));
        });
    }
    max.map_or(0, |max| max + 1)
}

fn is_gap(region: &Region) -> bool {
    matches!(region.node, Node::Leaf { .. })
        && region.visibility.collapsed
        && region.tags.iter().any(|tag| tag == "unchanged")
}

// ── gaps ──────────────────────────────────────────────────────────────────

/// Pairs of adjacent sibling gaps, keyed by the first one's id.
fn adjacent_gaps(regions: &[Region], out: &mut DftHashMap<u32, u32>) {
    for pair in regions.windows(2) {
        if is_gap(&pair[0]) && is_gap(&pair[1]) && pair[0].range.end == pair[1].range.start {
            out.insert(pair[0].alignment_id, pair[1].alignment_id);
        }
    }
    for region in regions {
        if let Node::Fold { children } = &region.node {
            adjacent_gaps(children, out);
        }
    }
}

fn merge_gaps(sides: &mut Pairing<Source>) {
    let (lhs_adjacent, rhs_adjacent, lhs_ids, rhs_ids) = {
        let mut lhs_adjacent = DftHashMap::default();
        let mut rhs_adjacent = DftHashMap::default();
        if let Some(lhs) = sides.lhs() {
            adjacent_gaps(&lhs.regions, &mut lhs_adjacent);
        }
        if let Some(rhs) = sides.rhs() {
            adjacent_gaps(&rhs.regions, &mut rhs_adjacent);
        }
        let ids = |source: Option<&Source>| {
            source
                .map(|source| super::ids(&source.regions))
                .unwrap_or_default()
        };
        (
            lhs_adjacent,
            rhs_adjacent,
            ids(sides.lhs()),
            ids(sides.rhs()),
        )
    };
    // A pair merges when the other side merges the same pair or has neither id.
    let agrees =
        |first: u32, second: u32, other: &DftHashMap<u32, u32>, other_ids: &DftHashSet<u32>| {
            other.get(&first) == Some(&second)
                || (!other_ids.contains(&first) && !other_ids.contains(&second))
        };
    let lhs_merges: DftHashSet<u32> = lhs_adjacent
        .iter()
        .filter(|(&first, &second)| agrees(first, second, &rhs_adjacent, &rhs_ids))
        .map(|(&first, _)| first)
        .collect();
    let rhs_merges: DftHashSet<u32> = rhs_adjacent
        .iter()
        .filter(|(&first, &second)| agrees(first, second, &lhs_adjacent, &lhs_ids))
        .map(|(&first, _)| first)
        .collect();
    if let Some(lhs) = lhs_mut(sides) {
        merge_gap_runs(&mut lhs.regions, &lhs_merges);
    }
    if let Some(rhs) = rhs_mut(sides) {
        merge_gap_runs(&mut rhs.regions, &rhs_merges);
    }
}

/// Fold each run of mergeable gaps into its first member, which keeps its
/// id so the pairing with the other side's merged gap holds.
fn merge_gap_runs(regions: &mut Vec<Region>, merges: &DftHashSet<u32>) {
    let mut merged: Vec<Region> = Vec::with_capacity(regions.len());
    for mut region in regions.drain(..) {
        if let Node::Fold { children } = &mut region.node {
            merge_gap_runs(children, merges);
        }
        match merged.last_mut() {
            Some(last) if merges.contains(&last.alignment_id) && is_gap(&region) => {
                last.range.end = region.range.end;
                let count = last.range.lines().len();
                last.visibility.label = format!("{count} unchanged lines");
                // The merged gap must itself continue a run only if it was
                // marked to; the mark lives on the first id, which it keeps.
            }
            _ => merged.push(region),
        }
    }
    *regions = merged;
}

// ── groups ────────────────────────────────────────────────────────────────

/// A collapsed fold labelled as removed lines only counts when nothing
/// under it is paired; a rewrite must not be swept into a "functions
/// removed" group.
fn removed_label(region: &Region) -> bool {
    region.visibility.label.ends_with("lines removed")
}

/// A unit is a collapsed fold, with the single-line leaf just before it
/// when there is one: the `def` or `fn` line whose body the fold hides.
fn unit_at(regions: &[Region], at: usize, other_ids: &DftHashSet<u32>) -> Option<usize> {
    let collapsed_fold = |region: &Region| {
        is_fold(region)
            && region.visibility.collapsed
            && (!removed_label(region) || one_sided(region, other_ids))
    };
    let region = regions.get(at)?;
    if collapsed_fold(region) {
        return Some(at);
    }
    let header = !is_fold(region) && region.range.lines().len() == 1;
    (header && regions.get(at + 1).is_some_and(collapsed_fold)).then_some(at + 1)
}

/// Lines a separator between units may span: blank lines, at most.
const MAX_SEPARATOR_LINES: usize = 2;

/// Maximal runs of two or more units among siblings, separated by at most
/// a couple of open leaf lines, as their child ids in document order.
fn collapsed_fold_runs(regions: &[Region], other_ids: &DftHashSet<u32>, out: &mut Vec<Vec<u32>>) {
    for region in regions {
        // Nothing under a collapsed fold is visible, so nothing there
        // needs grouping.
        if let (Node::Fold { children }, false) = (&region.node, region.visibility.collapsed) {
            collapsed_fold_runs(children, other_ids, out);
        }
    }
    let mut at = 0;
    while at < regions.len() {
        let Some(mut last) = unit_at(regions, at, other_ids) else {
            at += 1;
            continue;
        };
        let first = at;
        let mut units = 1;
        loop {
            let mut next = last + 1;
            let mut separator_lines = 0;
            while next < regions.len()
                && !is_fold(&regions[next])
                && !regions[next].visibility.collapsed
                && unit_at(regions, next, other_ids) != Some(next + 1)
            {
                separator_lines += regions[next].range.lines().len();
                next += 1;
            }
            if separator_lines > MAX_SEPARATOR_LINES {
                break;
            }
            match unit_at(regions, next, other_ids) {
                Some(end) => {
                    last = end;
                    units += 1;
                }
                None => break,
            }
        }
        if units >= 2 {
            out.push(
                regions[first..=last]
                    .iter()
                    .map(|r| r.alignment_id)
                    .collect(),
            );
        }
        at = last + 1;
    }
}

fn group_folds(sides: &mut Pairing<Source>, next_id: &mut u32) {
    let mut lhs_runs = Vec::new();
    let mut rhs_runs = Vec::new();
    let lhs_ids = sides.lhs().map(|lhs| ids(&lhs.regions)).unwrap_or_default();
    let rhs_ids = sides.rhs().map(|rhs| ids(&rhs.regions)).unwrap_or_default();
    if let Some(lhs) = sides.lhs() {
        collapsed_fold_runs(&lhs.regions, &rhs_ids, &mut lhs_runs);
    }
    if let Some(rhs) = sides.rhs() {
        collapsed_fold_runs(&rhs.regions, &lhs_ids, &mut rhs_runs);
    }
    // A run whose ids form the same run on the other side shares the group id.
    let mut group_ids: DftHashMap<Vec<u32>, u32> = DftHashMap::default();
    for run in lhs_runs.iter().chain(&rhs_runs) {
        if !group_ids.contains_key(run) {
            group_ids.insert(run.clone(), *next_id);
            *next_id += 1;
        }
    }
    let by_first = |runs: &[Vec<u32>]| -> DftHashMap<u32, (usize, u32)> {
        runs.iter()
            .map(|run| (run[0], (run.len(), group_ids[run])))
            .collect()
    };
    let lhs_by_first = by_first(&lhs_runs);
    let rhs_by_first = by_first(&rhs_runs);
    if let Some(lhs) = lhs_mut(sides) {
        wrap_runs(&mut lhs.regions, &lhs_by_first);
    }
    if let Some(rhs) = rhs_mut(sides) {
        wrap_runs(&mut rhs.regions, &rhs_by_first);
    }
}

fn wrap_runs(regions: &mut Vec<Region>, by_first: &DftHashMap<u32, (usize, u32)>) {
    for region in regions.iter_mut() {
        if let Node::Fold { children } = &mut region.node {
            wrap_runs(children, by_first);
        }
    }
    let mut wrapped: Vec<Region> = Vec::with_capacity(regions.len());
    let mut pending = std::mem::take(regions).into_iter();
    while let Some(region) = pending.next() {
        let Some(&(len, id)) = by_first.get(&region.alignment_id) else {
            wrapped.push(region);
            continue;
        };
        let mut children = vec![region];
        while children.len() < len {
            children.push(pending.next().expect("a run is contiguous"));
        }
        wrapped.push(group(id, children));
    }
    *regions = wrapped;
}

fn group(id: u32, children: Vec<Region>) -> Region {
    let folds: Vec<&Region> = children.iter().filter(|child| is_fold(child)).collect();
    let count = folds.len();
    let label = if folds
        .iter()
        .all(|fold| fold.visibility.label.ends_with("lines removed"))
    {
        format!("{count} functions removed")
    } else if folds
        .iter()
        .all(|fold| fold.visibility.label == "test body")
    {
        format!("{count} test bodies")
    } else if folds.iter().all(|fold| {
        fold.visibility
            .label
            .lines()
            .next()
            .is_some_and(|first| first.ends_with(" pseudocode"))
    }) {
        format!("{count} functions summarized")
    } else {
        format!("{count} folded regions")
    };
    Region {
        alignment_id: id,
        fold_state_id: id,
        range: SourceRange {
            start: children[0].range.start,
            end: children[children.len() - 1].range.end,
        },
        tags: vec!["group".to_owned()],
        visibility: Visibility {
            collapsed: true,
            label,
        },
        node: Node::Fold { children },
    }
}

fn lhs_mut(sides: &mut Pairing<Source>) -> Option<&mut Source> {
    match sides {
        Pairing::Both { lhs, .. } | Pairing::LeftOnly { lhs } => Some(lhs),
        Pairing::RightOnly { .. } => None,
    }
}

fn rhs_mut(sides: &mut Pairing<Source>) -> Option<&mut Source> {
    match sides {
        Pairing::Both { rhs, .. } | Pairing::RightOnly { rhs } => Some(rhs),
        Pairing::LeftOnly { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutate::collapse::DeletedBodies;
    use crate::mutate::summarize::tests::project;

    #[test]
    fn removed_groups_only_wrap_one_sided_folds() {
        use crate::protocol::SourcePos;
        let leaf = |id: u32, start: u32, end: u32| Region {
            alignment_id: id,
            fold_state_id: id,
            range: SourceRange {
                start: SourcePos {
                    line: start,
                    column: 0,
                },
                end: SourcePos {
                    line: end,
                    column: 0,
                },
            },
            tags: vec![],
            visibility: Visibility::default(),
            node: Node::Leaf { changed: vec![] },
        };
        let removed_fold = |id: u32, start: u32, end: u32, child: Region| Region {
            alignment_id: id,
            fold_state_id: id,
            range: SourceRange {
                start: SourcePos {
                    line: start,
                    column: 0,
                },
                end: SourcePos {
                    line: end,
                    column: 0,
                },
            },
            tags: vec!["body".to_owned(), "function".to_owned()],
            visibility: Visibility {
                collapsed: true,
                label: format!("{} lines removed", end - start),
            },
            node: Node::Fold {
                children: vec![child],
            },
        };
        // Two adjacent "removed" folds, but the first one's leaf is paired.
        let lhs = vec![
            leaf(1, 0, 1),
            removed_fold(2, 1, 6, leaf(3, 1, 6)),
            leaf(4, 6, 7),
            removed_fold(5, 7, 12, leaf(6, 7, 12)),
        ];
        let rhs = vec![leaf(3, 0, 5)];
        let source = |regions| Source {
            text: String::new(),
            syntax: vec![],
            regions,
        };
        let mut sides = Pairing::Both {
            lhs: source(lhs),
            rhs: source(rhs),
        };
        let file = crate::mutate::summarize::tests::project("m.py", "", "").0;
        GroupCollapsed.apply(&file, &mut sides).unwrap();
        let lhs = &sides.lhs().unwrap().regions;
        assert!(
            lhs.iter().all(|region| region.tags != ["group"]),
            "a rewrite next to a removal is not a run of removals"
        );
    }

    #[test]
    fn adjacent_deleted_bodies_are_grouped_under_one_collapsed_fold() {
        let before = "def a():\n    x()\n    y()\n    z()\n\ndef b():\n    x()\n    y()\n    z()\n\ndef c():\n    x()\n    y()\n    z()\n\nkeep = 1\n";
        let after = "keep = 1\n";
        let (file, mut sides) = project("m.py", before, after);
        DeletedBodies { min_lines: 3 }
            .apply(&file, &mut sides)
            .unwrap();
        GroupCollapsed.apply(&file, &mut sides).unwrap();
        let lhs = sides.lhs().unwrap();
        let group = lhs
            .regions
            .iter()
            .find(|region| region.tags == ["group"])
            .expect("a group fold");
        assert_eq!(group.visibility.label, "3 functions removed");
        assert!(group.visibility.collapsed);
        let Node::Fold { children } = &group.node else {
            panic!("a group is a fold");
        };
        for child in children {
            eprintln!(
                "child {} {:?} {:?} fold={}",
                child.alignment_id,
                child.range.lines(),
                child.visibility.label,
                is_fold(child)
            );
        }
        let folds: Vec<_> = children.iter().filter(|child| is_fold(child)).collect();
        assert_eq!(folds.len(), 3);
        assert!(folds.iter().all(|child| child.visibility.collapsed));
        // The group starts on the first `def` line and ends with the last body.
        assert_eq!(group.range.start.line, 0);
        assert_eq!(group.range.end, folds[2].range.end);
        let rhs_ids = super::super::ids(&sides.rhs().unwrap().regions);
        assert!(
            !rhs_ids.contains(&group.alignment_id),
            "a one-sided group has a fresh id"
        );
        let mut seen = DftHashSet::default();
        walk(&lhs.regions, &mut |region| {
            assert!(
                seen.insert(region.alignment_id),
                "duplicate id {}",
                region.alignment_id
            );
        });
    }

    #[test]
    fn a_single_collapsed_fold_is_left_alone() {
        let before = "def a():\n    x()\n    y()\n    z()\n\nkeep = 1\n";
        let after = "keep = 1\n";
        let (file, mut sides) = project("m.py", before, after);
        DeletedBodies { min_lines: 3 }
            .apply(&file, &mut sides)
            .unwrap();
        GroupCollapsed.apply(&file, &mut sides).unwrap();
        let mut groups = 0;
        walk(&sides.lhs().unwrap().regions, &mut |region| {
            groups += usize::from(region.tags == ["group"]);
        });
        assert_eq!(groups, 0);
    }

    #[test]
    fn adjacent_gaps_merge_on_both_sides_and_keep_their_pairing() {
        let mut sides = Pairing::Both {
            lhs: source(vec![gap(0, 0, 3), gap(1, 3, 7), leaf(2, 7, 8)]),
            rhs: source(vec![gap(0, 0, 3), gap(1, 3, 7), leaf(2, 7, 8)]),
        };
        merge_gaps(&mut sides);
        for source in [sides.lhs().unwrap(), sides.rhs().unwrap()] {
            assert_eq!(source.regions.len(), 2);
            assert_eq!(source.regions[0].alignment_id, 0);
            assert_eq!(source.regions[0].range.lines(), 0..7);
            assert_eq!(source.regions[0].visibility.label, "7 unchanged lines");
        }
    }

    #[test]
    fn gaps_do_not_merge_when_the_other_side_separates_them() {
        let mut sides = Pairing::Both {
            lhs: source(vec![gap(0, 0, 3), gap(1, 3, 7)]),
            rhs: source(vec![gap(0, 0, 3), leaf(5, 3, 4), gap(1, 4, 8)]),
        };
        merge_gaps(&mut sides);
        assert_eq!(sides.lhs().unwrap().regions.len(), 2);
        assert_eq!(sides.rhs().unwrap().regions.len(), 3);
    }

    fn source(regions: Vec<Region>) -> Source {
        Source {
            text: String::new(),
            syntax: Vec::new(),
            regions,
        }
    }

    fn range(start: u32, end: u32) -> SourceRange {
        SourceRange {
            start: crate::protocol::SourcePos {
                line: start,
                column: 0,
            },
            end: crate::protocol::SourcePos {
                line: end,
                column: 0,
            },
        }
    }

    fn gap(id: u32, start: u32, end: u32) -> Region {
        Region {
            alignment_id: id,
            fold_state_id: id,
            range: range(start, end),
            tags: vec!["unchanged".to_owned()],
            visibility: Visibility {
                collapsed: true,
                label: format!("{} unchanged lines", end - start),
            },
            node: Node::Leaf { changed: vec![] },
        }
    }

    fn leaf(id: u32, start: u32, end: u32) -> Region {
        Region {
            alignment_id: id,
            fold_state_id: id,
            range: range(start, end),
            tags: vec![],
            visibility: Visibility::default(),
            node: Node::Leaf { changed: vec![] },
        }
    }
}
