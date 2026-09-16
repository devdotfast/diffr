//! Carry out moves, in order. diffr carries every plugin's moves out with
//! this applier, and [`crate::Draft`] runs the same code on a copy, so a
//! plugin predicts exactly the trees and ids its moves leave.
//!
//! A move names a region by `id`, on the one side that holds it, or the file
//! by [`ROOT`]. An unknown `id` is an error.
//!
//! - `Cut { region, at }` splits a leaf into the lines before `at`, relative
//!   to its first line, and the lines from `at` on; `at` must fall strictly
//!   inside it. The leaf on the other side with its `alignment_id`, which
//!   has the same length, is cut at the same offset. The first piece on each
//!   side keeps its leaf's ids. The second piece takes a fresh `id` on each
//!   side and a fresh `alignment_id` shared by the two, and its
//!   `fold_state_id` is the `id` of the lhs piece, or of its only piece.
//!   Pieces keep the leaf's tags and visibility and the `changed` spans on
//!   their lines. A fold, or the file, cannot be cut.
//! - `JoinFolds { regions }` needs two or more region ids. Each side wraps the
//!   ones it holds, which must be two or more consecutive siblings under one
//!   parent, in a new fold spanning them, with a fresh `id`, no tags, and an
//!   open, unlabelled visibility. When both sides wrap, the two new folds
//!   share the lhs fold's `fold_state_id`, its `id`, so a plugin joins a run
//!   and the run matched with it on the other side in one move by listing
//!   both runs' ids. A side holding exactly one of the ids is an error, and
//!   so is an id no side holds.
//! - `LinkFoldState { regions }` needs two or more region ids. Every region
//!   in any of their fold states, on either side, takes the first region's
//!   `fold_state_id` and whether it starts collapsed.
//! - `SetCollapsed { region, collapsed }` sets whether every region sharing
//!   the region's `fold_state_id` starts collapsed, on both sides: they open
//!   and close together. On [`ROOT`] it sets whether the file starts hidden.
//! - `SetLabel { region, label }` sets the label of that region alone, or of
//!   the file on [`ROOT`]; `None` clears it.
//! - `SetTags { region, tags }` replaces that region's tags. The file's tags
//!   are its manifest entry's, fixed before any diff runs, so [`ROOT`] is an
//!   error.
//!
//! Fresh `id`s start above the largest `id` in the file (and above
//! [`ROOT`]) when a plugin's moves begin, and fresh `alignment_id`s above the
//! largest leaf `alignment_id`; each is handed out in the order the moves
//! need them, lhs before rhs.
use crate::tree::{walk, walk_mut, Node, Pairing, Region, Source};
use crate::types::{Move, Position, Range, Visibility, ROOT};
use anyhow::{bail, ensure};
use std::collections::BTreeSet;

/// Carries out one plugin's moves, handing out fresh ids as they need them.
pub struct Applier {
    fresh: Fresh,
}

impl Applier {
    /// An applier for moves on `sides` as they are now.
    pub fn new(sides: &Pairing<Source>) -> Self {
        Self {
            fresh: Fresh::of(sides),
        }
    }

    /// Carry out one move on `sides` and `file`, the file's own visibility.
    pub fn apply(
        &mut self,
        next: Move,
        sides: &mut Pairing<Source>,
        file: &mut Visibility,
    ) -> anyhow::Result<()> {
        match next {
            Move::Cut { region, at } => cut(sides, region, at, &mut self.fresh),
            Move::JoinFolds { regions } => join(sides, &regions, &mut self.fresh),
            Move::LinkFoldState { regions } => link(sides, &regions),
            Move::SetCollapsed {
                region: ROOT,
                collapsed,
            } => {
                file.collapsed = collapsed;
                Ok(())
            }
            Move::SetCollapsed { region, collapsed } => {
                let state = region_of(sides, region)?.fold_state_id;
                for tree in trees(sides) {
                    walk_mut(tree, &mut |region| {
                        if region.fold_state_id == state {
                            region.visibility.collapsed = collapsed;
                        }
                    });
                }
                Ok(())
            }
            Move::SetLabel {
                region: ROOT,
                label,
            } => {
                file.label = label.unwrap_or_default();
                Ok(())
            }
            Move::SetLabel { region, label } => {
                region_mut(sides, region)?.visibility.label = label.unwrap_or_default();
                Ok(())
            }
            Move::SetTags { region: ROOT, .. } => {
                bail!("the file's tags are its manifest entry's; only a region's tags can be set")
            }
            Move::SetTags { region, tags } => {
                region_mut(sides, region)?.tags = tags;
                Ok(())
            }
        }
    }
}

/// Carry out `moves` in order.
pub fn apply(
    moves: Vec<Move>,
    sides: &mut Pairing<Source>,
    file: &mut Visibility,
) -> anyhow::Result<()> {
    let mut applier = Applier::new(sides);
    for next in moves {
        applier.apply(next, sides, file)?;
    }
    Ok(())
}

fn trees(sides: &mut Pairing<Source>) -> Vec<&mut Vec<Region>> {
    match sides {
        Pairing::Both { lhs, rhs } => vec![&mut lhs.regions, &mut rhs.regions],
        Pairing::LeftOnly { lhs } => vec![&mut lhs.regions],
        Pairing::RightOnly { rhs } => vec![&mut rhs.regions],
    }
}

pub fn trees_ref(sides: &Pairing<Source>) -> Vec<&[Region]> {
    match sides {
        Pairing::Both { lhs, rhs } => vec![&lhs.regions, &rhs.regions],
        Pairing::LeftOnly { lhs } => vec![&lhs.regions],
        Pairing::RightOnly { rhs } => vec![&rhs.regions],
    }
}

/// The next unused `id` and leaf `alignment_id` in the file: what a plugin's
/// moves hand out, in order. A plugin that does not use [`crate::Draft`]
/// predicts the ids its cuts and joins create with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fresh {
    pub id: u32,
    pub alignment: u32,
}

impl Fresh {
    /// One above every `id`, and [`ROOT`], and one above every leaf
    /// `alignment_id`, in the file.
    pub fn of(sides: &Pairing<Source>) -> Self {
        let mut fresh = Self {
            id: ROOT + 1,
            alignment: 0,
        };
        for tree in trees_ref(sides) {
            walk(tree, &mut |region| {
                fresh.id = fresh.id.max(region.id + 1);
                if let Some(alignment_id) = region.alignment_id() {
                    fresh.alignment = fresh.alignment.max(alignment_id + 1);
                }
            });
        }
        fresh
    }

    /// Take the next `id`.
    pub fn id(&mut self) -> u32 {
        let id = self.id;
        self.id += 1;
        id
    }

    /// Take the next `alignment_id`.
    pub fn alignment(&mut self) -> u32 {
        let alignment = self.alignment;
        self.alignment += 1;
        alignment
    }
}

/// Child indices from the root down to the first region `is` accepts.
fn path_where(regions: &[Region], is: &impl Fn(&Region) -> bool) -> Option<Vec<usize>> {
    for (index, region) in regions.iter().enumerate() {
        if is(region) {
            return Some(vec![index]);
        }
        if let Node::Fold { children } = &region.node {
            if let Some(mut path) = path_where(children, is) {
                path.insert(0, index);
                return Some(path);
            }
        }
    }
    None
}

/// Child indices from the root down to the region with this `id`.
fn path_of(regions: &[Region], id: u32) -> Option<Vec<usize>> {
    path_where(regions, &|region| region.id == id)
}

/// The region at a path.
fn at<'a>(regions: &'a [Region], path: &[usize]) -> &'a Region {
    let (&index, rest) = path.split_first().expect("a path is never empty");
    match (rest.is_empty(), &regions[index].node) {
        (true, _) => &regions[index],
        (false, Node::Fold { children }) => at(children, rest),
        (false, Node::Leaf { .. }) => unreachable!("a path descends through folds"),
    }
}

/// The sibling list the last index of a path points into.
fn siblings<'a>(regions: &'a mut Vec<Region>, parent: &[usize]) -> &'a mut Vec<Region> {
    match parent.split_first() {
        None => regions,
        Some((&index, rest)) => match &mut regions[index].node {
            Node::Fold { children } => siblings(children, rest),
            Node::Leaf { .. } => unreachable!("a path descends through folds"),
        },
    }
}

/// The region with this `id`, on whichever side holds it.
fn region_of(sides: &Pairing<Source>, id: u32) -> anyhow::Result<&Region> {
    trees_ref(sides)
        .into_iter()
        .find_map(|tree| find(tree, id))
        .ok_or_else(|| anyhow::anyhow!("no region {id}"))
}

fn region_mut(sides: &mut Pairing<Source>, id: u32) -> anyhow::Result<&mut Region> {
    trees(sides)
        .into_iter()
        .find_map(|tree| find_mut(tree, id))
        .ok_or_else(|| anyhow::anyhow!("no region {id}"))
}

pub fn find(regions: &[Region], id: u32) -> Option<&Region> {
    regions.iter().find_map(|region| {
        if region.id == id {
            return Some(region);
        }
        match &region.node {
            Node::Fold { children } => find(children, id),
            Node::Leaf { .. } => None,
        }
    })
}

fn find_mut(regions: &mut [Region], id: u32) -> Option<&mut Region> {
    for region in regions {
        if region.id == id {
            return Some(region);
        }
        if let Node::Fold { children } = &mut region.node {
            if let Some(found) = find_mut(children, id) {
                return Some(found);
            }
        }
    }
    None
}

fn cut(sides: &mut Pairing<Source>, id: u32, offset: u32, fresh: &mut Fresh) -> anyhow::Result<()> {
    ensure!(id != ROOT, "the file cannot be cut; only a leaf can");
    let Some((side, path)) = trees(sides)
        .into_iter()
        .enumerate()
        .find_map(|(side, tree)| Some((side, path_of(tree, id)?)))
    else {
        bail!("no region {id}");
    };
    let (alignment, len) = {
        let leaf = at(trees(sides).swap_remove(side), &path);
        let Some(alignment) = leaf.alignment_id() else {
            bail!("region {id} is a fold; only a leaf can be cut");
        };
        (alignment, leaf.range.lines().len() as u32)
    };
    ensure!(
        0 < offset && offset < len,
        "line {offset} is not inside region {id}, which has {len} lines"
    );
    let piece_alignment = fresh.alignment();
    // The second pieces' fold state: the id of the first side's.
    let mut state = None;
    for (tree_side, tree) in trees(sides).into_iter().enumerate() {
        let path = if tree_side == side {
            path.clone()
        } else {
            match path_where(tree, &|region| region.alignment_id() == Some(alignment)) {
                Some(path) => path,
                None => continue,
            }
        };
        let (index, parent) = path.split_last().expect("a path is never empty");
        let list = siblings(tree, parent);
        ensure!(
            list[*index].range.lines().len() as u32 == len,
            "region {id} has a different length on each side"
        );
        let piece_id = fresh.id();
        let piece_state = *state.get_or_insert(piece_id);
        let leaf = list.remove(*index);
        let pieces = split(leaf, offset, piece_id, piece_alignment, piece_state);
        list.splice(*index..*index, pieces);
    }
    Ok(())
}

/// A leaf split at relative line `offset`. The second piece takes `id`,
/// `alignment_id` and `fold_state_id`.
fn split(leaf: Region, offset: u32, id: u32, alignment_id: u32, fold_state_id: u32) -> [Region; 2] {
    let Node::Leaf { changed, .. } = &leaf.node else {
        unreachable!("only leaves are cut");
    };
    let boundary = Position {
        line: leaf.range.start.line + offset,
        column: 0,
    };
    let piece = |range: Range, id: u32, alignment_id: u32, fold_state_id: u32| {
        let lines = range.lines();
        Region {
            id,
            fold_state_id,
            range,
            tags: leaf.tags.clone(),
            visibility: leaf.visibility.clone(),
            node: Node::Leaf {
                alignment_id,
                changed: changed
                    .iter()
                    .copied()
                    .filter(|span| lines.contains(&span.line))
                    .collect(),
            },
        }
    };
    let head = piece(
        Range {
            start: leaf.range.start,
            end: boundary,
        },
        leaf.id,
        leaf.alignment_id().expect("a leaf"),
        leaf.fold_state_id,
    );
    let tail = piece(
        Range {
            start: boundary,
            end: leaf.range.end,
        },
        id,
        alignment_id,
        fold_state_id,
    );
    [head, tail]
}

/// Two or more distinct region ids, none of them the file.
fn check_regions(ids: &[u32], what: &str) -> anyhow::Result<()> {
    ensure!(ids.len() >= 2, "{what} needs at least two regions");
    ensure!(!ids.contains(&ROOT), "{what} cannot include the file");
    ensure!(
        ids.iter().collect::<BTreeSet<_>>().len() == ids.len(),
        "{what} lists a region twice: {ids:?}"
    );
    Ok(())
}

fn link(sides: &mut Pairing<Source>, ids: &[u32]) -> anyhow::Result<()> {
    check_regions(ids, "a link")?;
    let first = region_of(sides, ids[0])?;
    let (state, collapsed) = (first.fold_state_id, first.visibility.collapsed);
    let states = ids
        .iter()
        .map(|id| Ok(region_of(sides, *id)?.fold_state_id))
        .collect::<anyhow::Result<BTreeSet<u32>>>()?;
    for tree in trees(sides) {
        walk_mut(tree, &mut |region| {
            if states.contains(&region.fold_state_id) {
                region.fold_state_id = state;
                region.visibility.collapsed = collapsed;
            }
        });
    }
    Ok(())
}

fn join(sides: &mut Pairing<Source>, ids: &[u32], fresh: &mut Fresh) -> anyhow::Result<()> {
    check_regions(ids, "a join")?;
    for id in ids {
        region_of(sides, *id)?;
    }
    let mut state = None;
    for tree in trees(sides) {
        let mut paths: Vec<Vec<usize>> = ids.iter().filter_map(|id| path_of(tree, *id)).collect();
        if paths.is_empty() {
            continue;
        }
        ensure!(
            paths.len() >= 2,
            "a side holds only one of the joined regions {ids:?}"
        );
        // Child-index paths sort in document order.
        paths.sort();
        let parent = &paths[0][..paths[0].len() - 1];
        let first = paths[0][paths[0].len() - 1];
        let adjacent = paths.iter().enumerate().all(|(offset, path)| {
            path.len() == paths[0].len()
                && &path[..path.len() - 1] == parent
                && path[path.len() - 1] == first + offset
        });
        ensure!(
            adjacent,
            "the joined regions {ids:?} are not consecutive siblings"
        );
        let parent = parent.to_vec();
        let list = siblings(tree, &parent);
        let children: Vec<Region> = list.drain(first..first + paths.len()).collect();
        let range = Range {
            start: children[0].range.start,
            end: children[children.len() - 1].range.end,
        };
        let id = fresh.id();
        let fold_state_id = *state.get_or_insert(id);
        list.insert(
            first,
            Region {
                id,
                fold_state_id,
                range,
                tags: Vec::new(),
                visibility: Visibility::default(),
                node: Node::Fold { children },
            },
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Span;

    fn range(start: u32, end: u32) -> Range {
        Range {
            start: Position {
                line: start,
                column: 0,
            },
            end: Position {
                line: end,
                column: 0,
            },
        }
    }

    /// A leaf whose `fold_state_id` is its `id`.
    fn leaf(id: u32, alignment: u32, start: u32, end: u32, changed: &[u32]) -> Region {
        Region {
            id,
            fold_state_id: id,
            range: range(start, end),
            tags: vec![],
            visibility: Visibility::default(),
            node: Node::Leaf {
                alignment_id: alignment,
                changed: changed
                    .iter()
                    .map(|&line| Span {
                        line,
                        start_column: 0,
                        end_column: 1,
                    })
                    .collect(),
            },
        }
    }

    /// A fold whose `fold_state_id` is its `id`.
    fn fold(id: u32, collapsed: bool, children: Vec<Region>) -> Region {
        Region {
            id,
            fold_state_id: id,
            range: Range {
                start: children[0].range.start,
                end: children[children.len() - 1].range.end,
            },
            tags: vec![],
            visibility: Visibility {
                collapsed,
                label: String::new(),
            },
            node: Node::Fold { children },
        }
    }

    /// The region sharing another's fold state: the second of a pair.
    fn in_state(mut region: Region, state: u32) -> Region {
        region.fold_state_id = state;
        region
    }

    fn source(regions: Vec<Region>) -> Source {
        Source {
            text: String::new(),
            regions,
        }
    }

    fn both(lhs: Vec<Region>, rhs: Vec<Region>) -> Pairing<Source> {
        Pairing::Both {
            lhs: source(lhs),
            rhs: source(rhs),
        }
    }

    fn run(moves: Vec<Move>, sides: &mut Pairing<Source>) -> anyhow::Result<Visibility> {
        let mut visibility = Visibility::default();
        apply(moves, sides, &mut visibility)?;
        Ok(visibility)
    }

    type Shape = (u32, Option<u32>, u32, u32, u32, bool, String);

    /// `(id, alignment_id, fold_state_id, start, end, collapsed, label)` in
    /// preorder.
    fn shape(regions: &[Region]) -> Vec<Shape> {
        let mut out = Vec::new();
        walk(regions, &mut |region| {
            out.push((
                region.id,
                region.alignment_id(),
                region.fold_state_id,
                region.range.start.line,
                region.range.end.line,
                region.visibility.collapsed,
                region.visibility.label.clone(),
            ))
        });
        out
    }

    fn sides_of(sides: &Pairing<Source>) -> (&Source, &Source) {
        let Pairing::Both { lhs, rhs } = sides else {
            panic!("both sides");
        };
        (lhs, rhs)
    }

    #[test]
    fn cutting_a_paired_leaf_cuts_both_sides_with_fresh_ids_and_a_shared_alignment() {
        // Leaves 2 (lhs) and 3 (rhs) are paired: alignment 1, fold state 2.
        // Cutting either side cuts both the same way.
        for target in [2, 3] {
            let mut sides = both(
                vec![leaf(1, 0, 0, 2, &[]), leaf(2, 1, 2, 8, &[3, 6])],
                vec![
                    in_state(leaf(3, 1, 0, 6, &[1, 4]), 2),
                    leaf(4, 2, 6, 7, &[]),
                ],
            );
            let moves = vec![
                Move::Cut {
                    region: target,
                    at: 2,
                },
                Move::Cut { region: 5, at: 2 },
            ];
            run(moves, &mut sides).unwrap();
            let (lhs, rhs) = sides_of(&sides);
            let open = String::new;
            assert_eq!(
                shape(&lhs.regions),
                [
                    (1, Some(0), 1, 0, 2, false, open()),
                    (2, Some(1), 2, 2, 4, false, open()),
                    (5, Some(3), 5, 4, 6, false, open()),
                    (7, Some(4), 7, 6, 8, false, open()),
                ],
                "target {target}"
            );
            assert_eq!(
                shape(&rhs.regions),
                [
                    (3, Some(1), 2, 0, 2, false, open()),
                    (6, Some(3), 5, 2, 4, false, open()),
                    (8, Some(4), 7, 4, 6, false, open()),
                    (4, Some(2), 4, 6, 7, false, open()),
                ],
                "target {target}"
            );
            let Node::Leaf { changed, .. } = &lhs.regions[3].node else {
                panic!("a leaf");
            };
            assert_eq!(
                changed.iter().map(|span| span.line).collect::<Vec<_>>(),
                [6]
            );
        }
    }

    #[test]
    fn cutting_a_one_sided_leaf_takes_fresh_ids() {
        let mut sides = both(
            vec![leaf(1, 0, 0, 1, &[]), leaf(2, 1, 1, 6, &[])],
            vec![in_state(leaf(3, 0, 0, 1, &[]), 1)],
        );
        run(vec![Move::Cut { region: 2, at: 1 }], &mut sides).unwrap();
        let (lhs, rhs) = sides_of(&sides);
        assert_eq!(
            shape(&lhs.regions),
            [
                (1, Some(0), 1, 0, 1, false, String::new()),
                (2, Some(1), 2, 1, 2, false, String::new()),
                (4, Some(2), 4, 2, 6, false, String::new()),
            ]
        );
        assert_eq!(rhs.regions.len(), 1);
    }

    #[test]
    fn set_collapsed_reaches_every_region_in_the_fold_state_and_labels_reach_one() {
        // Folds 2 (lhs) and 4 (rhs) are matched.
        let mut sides = both(
            vec![
                leaf(1, 0, 0, 2, &[]),
                fold(2, false, vec![leaf(3, 1, 2, 5, &[])]),
            ],
            vec![in_state(fold(4, false, vec![leaf(5, 2, 0, 4, &[])]), 2)],
        );
        let moves = vec![
            Move::LinkFoldState {
                regions: vec![2, 1],
            },
            Move::SetCollapsed {
                region: 4,
                collapsed: true,
            },
            Move::SetLabel {
                region: 2,
                label: Some("summary".to_owned()),
            },
        ];
        run(moves, &mut sides).unwrap();
        let (lhs, rhs) = sides_of(&sides);
        assert_eq!(
            shape(&lhs.regions),
            [
                (1, Some(0), 2, 0, 2, true, String::new()),
                (2, None, 2, 2, 5, true, "summary".to_owned()),
                (3, Some(1), 3, 2, 5, false, String::new()),
            ]
        );
        assert_eq!(
            shape(&rhs.regions)[0],
            (4, None, 2, 0, 4, true, String::new())
        );
        run(
            vec![
                Move::SetCollapsed {
                    region: 1,
                    collapsed: false,
                },
                Move::SetLabel {
                    region: 2,
                    label: None,
                },
            ],
            &mut sides,
        )
        .unwrap();
        let (lhs, rhs) = sides_of(&sides);
        assert!(lhs
            .regions
            .iter()
            .all(|region| region.visibility == Visibility::default()));
        assert!(!rhs.regions[0].visibility.collapsed);
    }

    #[test]
    fn a_link_takes_the_first_regions_fold_state_and_collapsed_state() {
        // Folds 1 (lhs) and 5 (rhs) are a matched pair, as are 3 (lhs) and
        // 8 (rhs).
        let tree = || {
            both(
                vec![
                    fold(1, false, vec![leaf(2, 0, 0, 3, &[])]),
                    fold(3, true, vec![leaf(4, 1, 3, 6, &[])]),
                ],
                vec![
                    in_state(fold(8, true, vec![leaf(7, 2, 0, 3, &[])]), 3),
                    in_state(fold(5, false, vec![leaf(6, 3, 3, 6, &[])]), 1),
                ],
            )
        };
        let mut sides = tree();
        run(
            vec![Move::LinkFoldState {
                regions: vec![7, 5],
            }],
            &mut sides,
        )
        .unwrap();
        let (lhs, rhs) = sides_of(&sides);
        assert_eq!(lhs.regions[0].fold_state_id, 7, "the pair stays together");
        assert_eq!(rhs.regions[1].fold_state_id, 7);
        assert_eq!(rhs.regions[0].fold_state_id, 3);

        let mut sides = tree();
        run(
            vec![Move::LinkFoldState {
                regions: vec![8, 1],
            }],
            &mut sides,
        )
        .unwrap();
        let (lhs, rhs) = sides_of(&sides);
        for region in [&lhs.regions[0], &lhs.regions[1], &rhs.regions[1]] {
            assert_eq!(
                (region.fold_state_id, region.visibility.collapsed),
                (3, true)
            );
        }
    }

    #[test]
    fn a_join_listing_both_sides_runs_wraps_each_with_one_fold_state() {
        // Folds 1 and 3 on the lhs are matched with 5 and 6 on the rhs; the
        // one-line leaves 2 (lhs) and 7 (rhs) between them are paired.
        let mut sides = both(
            vec![
                fold(1, true, vec![leaf(10, 0, 0, 3, &[])]),
                leaf(2, 1, 3, 4, &[]),
                fold(3, true, vec![leaf(11, 2, 4, 7, &[])]),
            ],
            vec![
                leaf(4, 3, 0, 1, &[]),
                in_state(fold(5, true, vec![leaf(12, 4, 1, 4, &[])]), 1),
                in_state(leaf(7, 1, 4, 5, &[]), 2),
                in_state(fold(6, true, vec![leaf(13, 5, 5, 8, &[])]), 3),
            ],
        );
        let moves = vec![Move::JoinFolds {
            regions: vec![1, 2, 3, 5, 7, 6],
        }];
        run(moves, &mut sides).unwrap();
        let (lhs, rhs) = sides_of(&sides);
        assert_eq!(lhs.regions.len(), 1);
        assert_eq!(
            shape(&lhs.regions)[0],
            (14, None, 14, 0, 7, false, String::new())
        );
        assert_eq!(rhs.regions.len(), 2);
        assert_eq!(
            shape(&rhs.regions[1..])[0],
            (15, None, 14, 1, 8, false, String::new())
        );
    }

    #[test]
    fn a_join_wraps_consecutive_siblings_on_each_side_that_holds_them() {
        let mut sides = both(
            vec![
                leaf(1, 0, 0, 1, &[]),
                fold(2, false, vec![leaf(3, 1, 1, 4, &[])]),
                leaf(4, 2, 4, 5, &[]),
                fold(5, true, vec![leaf(6, 3, 5, 8, &[])]),
            ],
            vec![in_state(leaf(7, 0, 0, 1, &[]), 1)],
        );
        let moves = vec![Move::JoinFolds {
            regions: vec![2, 4, 5],
        }];
        run(moves, &mut sides).unwrap();
        let (lhs, rhs) = sides_of(&sides);
        assert_eq!(lhs.regions.len(), 2);
        assert_eq!(
            shape(&lhs.regions[1..])[0],
            (8, None, 8, 1, 8, false, String::new())
        );
        assert_eq!(rhs.regions.len(), 1);
    }

    #[test]
    fn moves_that_cannot_be_carried_out_are_errors() {
        let tree = || {
            both(
                vec![
                    leaf(1, 0, 0, 1, &[]),
                    fold(2, false, vec![leaf(3, 1, 1, 4, &[])]),
                    leaf(4, 2, 4, 5, &[]),
                ],
                vec![
                    in_state(leaf(5, 0, 0, 1, &[]), 1),
                    in_state(leaf(6, 2, 1, 2, &[]), 4),
                ],
            )
        };
        let error = |next: Move| run(vec![next], &mut tree()).unwrap_err().to_string();
        assert!(error(Move::SetCollapsed {
            region: 9,
            collapsed: true
        })
        .contains("no region 9"));
        assert!(error(Move::Cut { region: 3, at: 3 }).contains("not inside region 3"));
        assert!(error(Move::Cut { region: 3, at: 0 }).contains("not inside region 3"));
        assert!(error(Move::Cut { region: 2, at: 1 }).contains("is a fold"));
        assert!(error(Move::Cut {
            region: ROOT,
            at: 1
        })
        .contains("the file cannot be cut"));
        assert!(error(Move::JoinFolds {
            regions: vec![1, 4]
        })
        .contains("not consecutive"));
        assert!(error(Move::JoinFolds {
            regions: vec![1, 2, 99]
        })
        .contains("no region 99"));
        assert!(error(Move::JoinFolds {
            regions: vec![ROOT, 1]
        })
        .contains("cannot include the file"));
        assert!(error(Move::JoinFolds {
            regions: vec![1, 2, 5]
        })
        .contains("holds only one"));
        assert!(error(Move::LinkFoldState { regions: vec![1] }).contains("at least two"));
        assert!(error(Move::LinkFoldState {
            regions: vec![1, 1]
        })
        .contains("twice"));
        assert!(error(Move::SetTags {
            region: ROOT,
            tags: vec![]
        })
        .contains("manifest"));
    }

    #[test]
    fn the_root_is_the_file() {
        let mut sides = both(
            vec![leaf(1, 0, 0, 1, &[])],
            vec![in_state(leaf(2, 0, 0, 1, &[]), 1)],
        );
        let visibility = run(
            vec![
                Move::SetLabel {
                    region: ROOT,
                    label: Some("first".to_owned()),
                },
                Move::SetCollapsed {
                    region: ROOT,
                    collapsed: true,
                },
                Move::SetLabel {
                    region: ROOT,
                    label: Some("Generated file".to_owned()),
                },
                Move::SetTags {
                    region: 2,
                    tags: vec!["mine:tag".to_owned()],
                },
            ],
            &mut sides,
        )
        .unwrap();
        assert_eq!(
            visibility,
            Visibility {
                collapsed: true,
                label: "Generated file".to_owned()
            }
        );
        let (lhs, rhs) = sides_of(&sides);
        assert_eq!(lhs.regions[0].visibility, Visibility::default());
        assert_eq!(rhs.regions[0].tags, ["mine:tag"]);
    }
}
