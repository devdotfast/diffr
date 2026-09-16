//! A plugin's moves, carried out on a copy of the region trees as the plugin
//! makes them. The copy lets a plugin name what its own moves create (the
//! piece a cut leaves, the fold a join adds) with the ids the applier will
//! give them, since both hand out ids the same way.
//!
//! The helpers here are the moves the bundled plugins make together:
//! collapsing a region under a label, linking regions, cutting lines out of
//! a leaf, and grouping siblings under a labelled fold.
use crate::apply::{find, trees_ref, Applier};
use crate::tree::{siblings_of, walk, Node, Pairing, Region, Source};
use crate::types::{Move, Visibility};
use anyhow::{anyhow, ensure};

pub struct Draft {
    /// The trees as the moves so far leave them. Only regions are copied.
    sides: Pairing<Source>,
    file: Visibility,
    applier: Applier,
    moves: Vec<Move>,
}

impl Draft {
    pub fn new(sides: &Pairing<Source>) -> Self {
        let sides = sides.clone().map(|source| Source {
            text: String::new(),
            regions: source.regions,
        });
        Self {
            applier: Applier::new(&sides),
            sides,
            file: Visibility::default(),
            moves: Vec::new(),
        }
    }

    /// Carry out `next` on the copy and keep it. A move the applier refuses
    /// is an error, as it would be in the pipeline.
    pub fn push(&mut self, next: Move) -> anyhow::Result<()> {
        self.applier
            .apply(next.clone(), &mut self.sides, &mut self.file)?;
        self.moves.push(next);
        Ok(())
    }

    pub fn into_moves(self) -> Vec<Move> {
        self.moves
    }

    fn region(&self, id: u32) -> anyhow::Result<&Region> {
        trees_ref(&self.sides)
            .into_iter()
            .find_map(|tree| find(tree, id))
            .ok_or_else(|| anyhow!("no region {id}"))
    }

    fn regions(&self, visit: &mut impl FnMut(&Region)) {
        for tree in trees_ref(&self.sides) {
            walk(tree, visit);
        }
    }

    /// The leaf on the other side whose rows pair with the leaf `id`.
    pub fn paired_leaf(&self, id: u32) -> anyhow::Result<Option<u32>> {
        let alignment = self.region(id)?.alignment_id();
        let mut paired = None;
        self.regions(&mut |region| {
            if region.id != id && alignment.is_some() && region.alignment_id() == alignment {
                paired = Some(region.id);
            }
        });
        Ok(paired)
    }

    /// Start `id` collapsed behind `label`, and with it every region in its
    /// fold state. The leaf paired with a leaf `id` takes the label too. Any
    /// other region in the fold state that was open loses its label, so it
    /// reads as part of `id`'s row rather than as its own placeholder.
    pub fn collapse(&mut self, id: u32, label: String) -> anyhow::Result<()> {
        let target = self.region(id)?;
        let (state, alignment) = (target.fold_state_id, target.alignment_id());
        let (mut named, mut cleared) = (vec![id], Vec::new());
        self.regions(&mut |region| {
            if region.fold_state_id != state || region.id == id {
                return;
            }
            if alignment.is_some() && region.alignment_id() == alignment {
                named.push(region.id);
            } else if !region.visibility.collapsed && !region.visibility.label.is_empty() {
                cleared.push(region.id);
            }
        });
        self.push(Move::SetCollapsed {
            region: id,
            collapsed: true,
        })?;
        for region in named {
            self.push(Move::SetLabel {
                region,
                label: Some(label.clone()),
            })?;
        }
        for region in cleared {
            self.push(Move::SetLabel {
                region,
                label: None,
            })?;
        }
        Ok(())
    }

    /// Open and close `ids` together, collapsed if any region in their fold
    /// states starts collapsed. A region that was open and so starts
    /// collapsed loses its label, as in [`Draft::collapse`].
    pub fn link(&mut self, ids: &[u32]) -> anyhow::Result<()> {
        let states = ids
            .iter()
            .map(|id| Ok(self.region(*id)?.fold_state_id))
            .collect::<anyhow::Result<Vec<u32>>>()?;
        let first_collapsed = self.region(ids[0])?.visibility.collapsed;
        let (mut collapsed, mut cleared) = (false, Vec::new());
        self.regions(&mut |region| {
            if !states.contains(&region.fold_state_id) {
                return;
            }
            collapsed |= region.visibility.collapsed;
            if !region.visibility.collapsed && !region.visibility.label.is_empty() {
                cleared.push(region.id);
            }
        });
        self.push(Move::LinkFoldState {
            regions: ids.to_vec(),
        })?;
        if !collapsed {
            return Ok(());
        }
        if !first_collapsed {
            self.push(Move::SetCollapsed {
                region: ids[0],
                collapsed: true,
            })?;
        }
        for region in cleared {
            self.push(Move::SetLabel {
                region,
                label: None,
            })?;
        }
        Ok(())
    }

    /// Cut the lines `start..end`, relative to the leaf `id`'s first line
    /// and half-open, out of it (and so out of the leaf paired with it),
    /// returning the `id` of the piece on `id`'s side that holds them. The
    /// leaf keeps its `id` for the lines before `start`.
    pub fn cut_lines(&mut self, id: u32, start: u32, end: u32) -> anyhow::Result<u32> {
        let leaf = self.region(id)?;
        ensure!(
            leaf.alignment_id().is_some(),
            "region {id} is a fold; only a leaf can be cut to lines"
        );
        let len = leaf.range.lines().len() as u32;
        ensure!(
            start < end && end <= len,
            "lines {start}..{end} are outside region {id}, which has {len} lines"
        );
        let mut piece = id;
        if start > 0 {
            self.push(Move::Cut {
                region: id,
                at: start,
            })?;
            piece = self.next_sibling(id)?;
        }
        if end < len {
            self.push(Move::Cut {
                region: piece,
                at: end - start,
            })?;
        }
        Ok(piece)
    }

    /// The `id` of the region just after `id` among its siblings.
    fn next_sibling(&self, id: u32) -> anyhow::Result<u32> {
        trees_ref(&self.sides)
            .into_iter()
            .find_map(|tree| {
                let siblings = siblings_of(tree, id)?;
                let index = siblings.iter().position(|region| region.id == id)?;
                siblings.get(index + 1).map(|region| region.id)
            })
            .ok_or_else(|| anyhow!("region {id} has no next sibling"))
    }

    /// Wrap `ids`, consecutive siblings on each side that holds them, in a
    /// new fold on each such side, starting collapsed behind `label`.
    pub fn group(&mut self, ids: Vec<u32>, label: String) -> anyhow::Result<()> {
        self.push(Move::JoinFolds {
            regions: ids.clone(),
        })?;
        let mut folds = Vec::new();
        for tree in trees_ref(&self.sides) {
            let mut parent = None;
            walk(tree, &mut |region| {
                if let Node::Fold { children } = &region.node {
                    if children.iter().any(|child| ids.contains(&child.id)) {
                        parent = Some(region.id);
                    }
                }
            });
            folds.extend(parent);
        }
        let first = *folds.first().expect("a join adds a fold on some side");
        self.push(Move::SetCollapsed {
            region: first,
            collapsed: true,
        })?;
        for region in folds {
            self.push(Move::SetLabel {
                region,
                label: Some(label.clone()),
            })?;
        }
        Ok(())
    }
}
