//! Region trees: each side's flat preorder list rebuilt as a tree, the
//! records rebuilt from a tree, and the helpers plugins read trees with.
use crate::types::{self, Range, Span, Visibility, ROOT};
use std::collections::BTreeSet;

/// Which sides a thing exists on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pairing<T> {
    Both { lhs: T, rhs: T },
    LeftOnly { lhs: T },
    RightOnly { rhs: T },
}

impl<T> Pairing<T> {
    /// The same sides, each value transformed by `f`.
    pub fn map<U>(self, mut f: impl FnMut(T) -> U) -> Pairing<U> {
        match self {
            Self::Both { lhs, rhs } => Pairing::Both {
                lhs: f(lhs),
                rhs: f(rhs),
            },
            Self::LeftOnly { lhs } => Pairing::LeftOnly { lhs: f(lhs) },
            Self::RightOnly { rhs } => Pairing::RightOnly { rhs: f(rhs) },
        }
    }

    pub fn lhs(&self) -> Option<&T> {
        match self {
            Self::Both { lhs, .. } | Self::LeftOnly { lhs } => Some(lhs),
            Self::RightOnly { .. } => None,
        }
    }

    pub fn rhs(&self) -> Option<&T> {
        match self {
            Self::Both { rhs, .. } | Self::RightOnly { rhs } => Some(rhs),
            Self::LeftOnly { .. } => None,
        }
    }

    /// Every side that exists, lhs first.
    pub fn sides(&self) -> Vec<&T> {
        match self {
            Self::Both { lhs, rhs } => vec![lhs, rhs],
            Self::LeftOnly { lhs } => vec![lhs],
            Self::RightOnly { rhs } => vec![rhs],
        }
    }

    /// Every side that exists, lhs first.
    pub fn sides_mut(&mut self) -> Vec<&mut T> {
        match self {
            Self::Both { lhs, rhs } => vec![lhs, rhs],
            Self::LeftOnly { lhs } => vec![lhs],
            Self::RightOnly { rhs } => vec![rhs],
        }
    }
}

/// One region of a side's tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    pub id: u32,
    pub fold_state_id: u32,
    pub range: Range,
    pub tags: Vec<String>,
    pub visibility: Visibility,
    pub node: Node,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Leaf {
        alignment_id: u32,
        changed: Vec<Span>,
    },
    Fold {
        children: Vec<Region>,
    },
}

impl Region {
    /// A leaf's row alignment; a fold has none.
    pub fn alignment_id(&self) -> Option<u32> {
        match self.node {
            Node::Leaf { alignment_id, .. } => Some(alignment_id),
            Node::Fold { .. } => None,
        }
    }
}

/// One side of the diffed file as a tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub text: String,
    /// The largest regions, in order.
    pub regions: Vec<Region>,
}

impl Source {
    /// Rebuild a side's tree from its preorder list with parent ids.
    pub fn from_record(side: &types::Source) -> anyhow::Result<Self> {
        fn children(
            regions: &mut std::iter::Peekable<std::slice::Iter<'_, types::Region>>,
            parent: u32,
        ) -> Vec<Region> {
            let mut out = Vec::new();
            while let Some(region) = regions.next_if(|region| region.parent == parent) {
                let node = match &region.kind {
                    types::Kind::Leaf(leaf) => Node::Leaf {
                        alignment_id: leaf.alignment_id,
                        changed: leaf.changed.clone(),
                    },
                    types::Kind::Fold => Node::Fold {
                        children: children(regions, region.id),
                    },
                };
                out.push(Region {
                    id: region.id,
                    fold_state_id: region.fold_state_id,
                    range: region.range,
                    tags: region.tags.clone(),
                    visibility: region.visibility.clone(),
                    node,
                });
            }
            out
        }
        let mut regions = side.regions.iter().peekable();
        let tree = children(&mut regions, ROOT);
        if let Some(stray) = regions.next() {
            anyhow::bail!(
                "region {} names parent {}, which does not precede it",
                stray.id,
                stray.parent
            );
        }
        Ok(Self {
            text: side.text.clone(),
            regions: tree,
        })
    }

    /// The side as a record: the tree flattened in preorder with parent ids.
    pub fn to_record(&self) -> types::Source {
        fn flatten(regions: &[Region], parent: u32, out: &mut Vec<types::Region>) {
            for region in regions {
                let kind = match &region.node {
                    Node::Leaf {
                        alignment_id,
                        changed,
                    } => types::Kind::Leaf(types::Leaf {
                        alignment_id: *alignment_id,
                        changed: changed.clone(),
                    }),
                    Node::Fold { .. } => types::Kind::Fold,
                };
                out.push(types::Region {
                    id: region.id,
                    parent,
                    fold_state_id: region.fold_state_id,
                    range: region.range,
                    tags: region.tags.clone(),
                    visibility: region.visibility.clone(),
                    kind,
                });
                if let Node::Fold { children } = &region.node {
                    flatten(children, region.id, out);
                }
            }
        }
        let mut regions = Vec::new();
        flatten(&self.regions, ROOT, &mut regions);
        types::Source {
            text: self.text.clone(),
            regions,
        }
    }
}

/// The contract's sides as the trees the SDK works on. diffr calls this once
/// on the way in, so a plugin is handed the pairing rather than the record.
pub fn sides(sides: &types::SourceSides) -> anyhow::Result<Pairing<Source>> {
    Ok(match sides {
        types::SourceSides::Both((lhs, rhs)) => Pairing::Both {
            lhs: Source::from_record(lhs)?,
            rhs: Source::from_record(rhs)?,
        },
        types::SourceSides::LeftOnly(lhs) => Pairing::LeftOnly {
            lhs: Source::from_record(lhs)?,
        },
        types::SourceSides::RightOnly(rhs) => Pairing::RightOnly {
            rhs: Source::from_record(rhs)?,
        },
    })
}

// ── region helpers ────────────────────────────────────────────────────────

pub fn walk(regions: &[Region], visit: &mut impl FnMut(&Region)) {
    for region in regions {
        visit(region);
        if let Node::Fold { children } = &region.node {
            walk(children, visit);
        }
    }
}

pub fn walk_mut(regions: &mut [Region], visit: &mut impl FnMut(&mut Region)) {
    for region in regions {
        visit(region);
        if let Node::Fold { children } = &mut region.node {
            walk_mut(children, visit);
        }
    }
}

/// What the other side holds, to tell one side's paired regions from
/// one-sided ones. A leaf is paired when the other side holds its
/// `alignment_id`: its rows line up with a leaf there. A fold is paired when
/// a region on the other side shares its `fold_state_id`: the fold the
/// matcher paired it with, or a region linked to that fold. Only the other
/// side counts, since a link merges fold states within a side as well.
#[derive(Default)]
pub struct OtherSide {
    leaf_ids: BTreeSet<u32>,
    fold_state_ids: BTreeSet<u32>,
}

impl OtherSide {
    /// What the other side, whose regions are `other`, holds.
    pub fn of(other: &[Region]) -> Self {
        let mut side = Self::default();
        walk(other, &mut |region| {
            if let Some(alignment_id) = region.alignment_id() {
                side.leaf_ids.insert(alignment_id);
            }
            side.fold_state_ids.insert(region.fold_state_id);
        });
        side
    }

    /// Whether `region`, on the side opposite this one, is paired.
    pub fn pairs(&self, region: &Region) -> bool {
        match region.node {
            Node::Leaf { alignment_id, .. } => self.leaf_ids.contains(&alignment_id),
            Node::Fold { .. } => self.fold_state_ids.contains(&region.fold_state_id),
        }
    }
}

/// True when no leaf under `region`, itself included, is paired with the
/// other side.
///
/// Newness is the leaves' answer. A fold covers its body alone — the
/// header line that opens it belongs to the leaf before it — so the leaves
/// inside a body fold are the body, and a body grown in place, whose lines
/// still align, is a rewrite rather than a removal. A fold state, on the
/// other hand, says only whether the matcher paired the two nodes, and a
/// file diffed by line pairs no folds at all while its lines still align.
/// So a fold is new, or deleted, exactly when nothing inside it lines up
/// with the other side.
pub fn one_sided(region: &Region, other: &OtherSide) -> bool {
    let mut paired = false;
    walk(std::slice::from_ref(region), &mut |inner| {
        paired |= matches!(inner.node, Node::Leaf { .. }) && other.pairs(inner);
    });
    !paired
}

/// The lines a region spans: for a fold, exactly the lines collapsing it
/// hides. A body fold spans its body alone, starting the line after the
/// `{` or `:` that opens it.
pub fn line_count(region: &Region) -> usize {
    region.range.lines().len()
}

pub fn is_fold(region: &Region) -> bool {
    matches!(region.node, Node::Fold { .. })
}

pub fn has_tag(region: &Region, tag: &str) -> bool {
    region.tags.iter().any(|own| own == tag)
}

/// How many lines (attributes and the signature) may sit between a
/// docstring and the line its function's body opens on.
const MAX_SIGNATURE_LINES: usize = 12;

/// The `id` of the docstring of `body`, a function body fold on `side`, as
/// `plugin`'s queries tag it (`<plugin>:docstring`, from
/// `plugins/shared/queries/<language>-docstrings.scm`): the body's first
/// fold child when it starts where the body does (Python), or else the fold
/// just before the body in document order, with
/// only a signature's worth of lines between them. The search runs back
/// through the body's siblings and then out through the folds that enclose
/// it, so a fold wrapping the whole function (another plugin's scope) does
/// not hide the docstring above it; it stops at a fold carrying one of
/// `plugin`'s own tags, such as the body of an enclosing function. A plugin
/// that collapses a body links it to its docstring, so the two open and
/// close together.
pub fn docstring_of(side: &Source, body: &Region, plugin: &str) -> Option<u32> {
    let tag = format!("{plugin}:docstring");
    let own = format!("{plugin}:");
    if let Node::Fold { children } = &body.node {
        if let Some(first) = children
            .iter()
            .find(|child| is_fold(child))
            .filter(|first| has_tag(first, &tag) && first.range.start.line == body.range.start.line)
        {
            return Some(first.id);
        }
    }
    let path = path_to(&side.regions, body.id).expect("the body is on this side");
    let mut between = 0;
    // From the body's own siblings outwards: at each level, the regions
    // before the one on the path.
    let mut level: &[Region] = &side.regions;
    let mut levels = Vec::new();
    for &index in &path {
        levels.push((level, index));
        if let Node::Fold { children } = &level[index].node {
            level = children;
        }
    }
    for (depth, (siblings, index)) in levels.iter().enumerate().rev() {
        if depth + 1 < path.len() {
            // Leaving the fold at `siblings[index]` for the regions before it.
            let parent = &siblings[*index];
            if parent.tags.iter().any(|tag| tag.starts_with(&own)) {
                return None;
            }
        }
        for region in siblings[..*index].iter().rev() {
            if is_fold(region) {
                return has_tag(region, &tag).then_some(region.id);
            }
            between += line_count(region);
            if between > MAX_SIGNATURE_LINES {
                return None;
            }
        }
    }
    None
}

/// Child indices from the root down to the region with this `id`.
pub fn path_to(regions: &[Region], id: u32) -> Option<Vec<usize>> {
    for (index, region) in regions.iter().enumerate() {
        if region.id == id {
            return Some(vec![index]);
        }
        if let Node::Fold { children } = &region.node {
            if let Some(mut path) = path_to(children, id) {
                path.insert(0, index);
                return Some(path);
            }
        }
    }
    None
}

/// The sibling list holding the region with this `id`.
pub fn siblings_of(regions: &[Region], id: u32) -> Option<&[Region]> {
    if regions.iter().any(|region| region.id == id) {
        return Some(regions);
    }
    regions.iter().find_map(|region| match &region.node {
        Node::Fold { children } => siblings_of(children, id),
        Node::Leaf { .. } => None,
    })
}

/// The before side, and what the after side holds.
pub fn before_and_after_ids(sides: &Pairing<Source>) -> Option<(&Source, OtherSide)> {
    match sides {
        Pairing::Both { lhs, rhs } => Some((lhs, OtherSide::of(&rhs.regions))),
        Pairing::LeftOnly { lhs } => Some((lhs, OtherSide::default())),
        Pairing::RightOnly { .. } => None,
    }
}

/// Each side that exists, with what the other side holds.
pub fn sides_with_other_ids(sides: &Pairing<Source>) -> Vec<(&Source, OtherSide)> {
    match sides {
        Pairing::Both { lhs, rhs } => vec![
            (lhs, OtherSide::of(&rhs.regions)),
            (rhs, OtherSide::of(&lhs.regions)),
        ],
        Pairing::LeftOnly { lhs } => vec![(lhs, OtherSide::default())],
        Pairing::RightOnly { rhs } => vec![(rhs, OtherSide::default())],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Position;

    /// A one-line leaf, or a fold holding one, with `id`. A leaf's
    /// `alignment_id` is `alignment`; the fold's child takes ids of its own.
    fn region(id: u32, alignment: u32, fold_state_id: u32, line: u32, fold: bool) -> Region {
        let range = Range {
            start: Position { line, column: 0 },
            end: Position {
                line: line + 1,
                column: 0,
            },
        };
        let leaf = Region {
            id,
            fold_state_id,
            range,
            tags: vec![],
            visibility: Visibility::default(),
            node: Node::Leaf {
                alignment_id: alignment,
                changed: vec![],
            },
        };
        if !fold {
            return leaf;
        }
        Region {
            node: Node::Fold {
                children: vec![Region {
                    id: id + 100,
                    fold_state_id: id + 100,
                    node: Node::Leaf {
                        alignment_id: alignment + 100,
                        changed: vec![],
                    },
                    ..leaf.clone()
                }],
            },
            ..leaf
        }
    }

    #[test]
    fn leaves_pair_by_alignment_id_and_folds_by_fold_state_on_the_other_side() {
        // lhs: a paired leaf (1), a matched fold (2), and an unmatched fold
        // (3) linked on this side with a leaf (4). rhs: the leaf paired with
        // 1, which shares its alignment id (5), and the fold matched with 2,
        // which shares its fold state (6).
        let lhs = vec![
            region(1, 1, 1, 0, false),
            region(2, 2, 2, 1, true),
            region(3, 3, 3, 2, true),
            region(4, 4, 3, 3, false),
        ];
        let rhs = vec![region(5, 1, 1, 0, false), region(6, 6, 2, 1, true)];
        let from_lhs = OtherSide::of(&rhs);
        let from_rhs = OtherSide::of(&lhs);
        assert!(from_lhs.pairs(&lhs[0]));
        assert!(from_lhs.pairs(&lhs[1]), "the matched fold is paired");
        assert!(
            !from_lhs.pairs(&lhs[2]),
            "a fold state shared only on its own side does not pair"
        );
        assert!(
            !from_lhs.pairs(&lhs[3]),
            "a leaf pairs by alignment id alone"
        );
        assert!(from_rhs.pairs(&rhs[0]));
        assert!(from_rhs.pairs(&rhs[1]));
    }

    #[test]
    fn newness_comes_from_the_leaves_not_the_fold_state() {
        // A fold matched across sides (both in fold state 2) whose leaves
        // align with nothing there: nothing inside it survived, so it is
        // one-sided, whatever the matcher made of the two nodes.
        let moved = region(2, 2, 2, 1, true);
        let elsewhere = region(6, 6, 2, 1, true);
        let other = OtherSide::of(std::slice::from_ref(&elsewhere));
        assert!(other.pairs(&moved), "the fold itself is matched");
        assert!(one_sided(&moved, &other));
        // The same fold, with a leaf inside it that does align: a rewrite.
        let kept = region(6, 2, 9, 1, true);
        let other = OtherSide::of(std::slice::from_ref(&kept));
        assert!(!other.pairs(&moved), "the folds are not matched");
        assert!(!one_sided(&moved, &other));
    }

    #[test]
    fn a_tree_survives_the_round_trip_through_its_record() {
        let source = Source {
            text: "a\nb\nc\n".to_owned(),
            regions: vec![region(1, 1, 1, 0, false), region(2, 2, 2, 1, true)],
        };
        let record = source.to_record();
        assert_eq!(
            record
                .regions
                .iter()
                .map(|region| (region.id, region.parent))
                .collect::<Vec<_>>(),
            [(1, ROOT), (2, ROOT), (102, 2)]
        );
        assert_eq!(Source::from_record(&record).unwrap(), source);
    }
}
