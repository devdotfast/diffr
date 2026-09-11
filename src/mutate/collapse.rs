//! Rules that start things collapsed: whole files by category, deleted
//! function bodies, and the middle of large removed stretches.
use super::group::next_id;
use super::{collapse, ids, is_fold, line_count, walk_mut, FileMutation, FoldMutation};
use crate::category;
use crate::hash::DftHashSet;
use crate::protocol::{
    FileChange, Node, Pairing, Problem, Region, Source, SourcePos, SourceRange, Visibility,
};

/// Generated and test files start hidden behind a placeholder.
pub(crate) struct HiddenCategories {
    pub(crate) generated: bool,
    pub(crate) tests: bool,
}

impl FileMutation for HiddenCategories {
    fn apply(&self, file: &mut FileChange) -> Result<(), Problem> {
        let label = match file.category.as_deref() {
            Some(category::GENERATED) if self.generated => "Generated file · hidden by default",
            Some(category::TEST) if self.tests => "Test file · hidden by default",
            _ => return Ok(()),
        };
        file.visibility.collapsed = true;
        file.visibility.label = label.to_owned();
        Ok(())
    }
}

/// Deleted bodies of at least `min_lines` start collapsed with a line count.
/// The header line stays visible by fold semantics.
pub(crate) struct DeletedBodies {
    pub(crate) min_lines: usize,
}

impl FoldMutation for DeletedBodies {
    fn apply(&self, _file: &FileChange, sides: &mut Pairing<Source>) -> Result<(), Problem> {
        let rhs_ids = sides.rhs().map(|rhs| ids(&rhs.regions)).unwrap_or_default();
        let Some(lhs) = lhs_mut(sides) else {
            return Ok(());
        };
        walk_mut(&mut lhs.regions, &mut |region| {
            if is_fold(region)
                && region.tags.iter().any(|tag| tag == "body")
                && !rhs_ids.contains(&region.id)
                && line_count(region) >= self.min_lines
            {
                let count = line_count(region);
                collapse(region, format!("{count} lines removed"));
            }
        });
        Ok(())
    }
}

/// Removed stretches with no counterpart and at least `min_lines` lines are
/// split into three leaves: the first line open, the middle collapsed with
/// a line count, the last line open, so the reader still sees red at both
/// ends. Leaves under a fold that already starts collapsed are left alone.
pub(crate) struct RemovedRuns {
    pub(crate) min_lines: usize,
}

impl FoldMutation for RemovedRuns {
    fn apply(&self, _file: &FileChange, sides: &mut Pairing<Source>) -> Result<(), Problem> {
        let rhs_ids = sides.rhs().map(|rhs| ids(&rhs.regions)).unwrap_or_default();
        let mut next_id = next_id(sides);
        let Some(lhs) = lhs_mut(sides) else {
            return Ok(());
        };
        // A leaf needs a first, a middle, and a last line to split.
        let threshold = self.min_lines.max(3);
        split_removed_runs(&mut lhs.regions, &rhs_ids, threshold, false, &mut next_id);
        Ok(())
    }
}

fn split_removed_runs(
    regions: &mut Vec<Region>,
    rhs_ids: &DftHashSet<u32>,
    threshold: usize,
    under_collapsed: bool,
    next_id: &mut u32,
) {
    let mut out = Vec::with_capacity(regions.len());
    for mut region in regions.drain(..) {
        let collapsed = under_collapsed || region.visibility.collapsed;
        let splits =
            !under_collapsed && !rhs_ids.contains(&region.id) && line_count(&region) >= threshold;
        match &mut region.node {
            Node::Fold { children } => {
                split_removed_runs(children, rhs_ids, threshold, collapsed, next_id);
                out.push(region);
            }
            Node::Leaf { .. } if splits => out.extend(split_leaf(region, next_id)),
            Node::Leaf { .. } => out.push(region),
        }
    }
    *regions = out;
}

/// First line, collapsed middle, last line. The first piece keeps the
/// leaf's id; the others take fresh ones. Every piece is one-sided, so
/// nothing needs pairing.
fn split_leaf(region: Region, next_id: &mut u32) -> Vec<Region> {
    let Node::Leaf { changed } = region.node else {
        unreachable!("only leaves are split");
    };
    let lines = region.range.lines();
    let (first, last) = (lines.start, lines.end - 1);
    let at = |line: u32| SourcePos { line, column: 0 };
    let piece = |id: u32, range: SourceRange, tags: Vec<String>, visibility: Visibility| {
        let changed = changed
            .iter()
            .copied()
            .filter(|span| range.lines().contains(&span.line))
            .collect();
        Region {
            id,
            range,
            tags,
            visibility,
            node: Node::Leaf { changed },
        }
    };
    let middle_id = *next_id;
    let last_id = *next_id + 1;
    *next_id += 2;
    let hidden = last - first - 1;
    vec![
        piece(
            region.id,
            SourceRange {
                start: region.range.start,
                end: at(first + 1),
            },
            region.tags.clone(),
            region.visibility.clone(),
        ),
        piece(
            middle_id,
            SourceRange {
                start: at(first + 1),
                end: at(last),
            },
            vec!["removed".to_owned()],
            Visibility {
                collapsed: true,
                label: format!("{hidden} lines removed"),
            },
        ),
        piece(
            last_id,
            SourceRange {
                start: at(last),
                end: region.range.end,
            },
            region.tags,
            Visibility::default(),
        ),
    ]
}

fn lhs_mut(sides: &mut Pairing<Source>) -> Option<&mut Source> {
    match sides {
        Pairing::Both { lhs, .. } | Pairing::LeftOnly { lhs } => Some(lhs),
        Pairing::RightOnly { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutate::walk;
    use crate::protocol::{FileRef, FileStatus, Visibility};

    fn manifest(category: Option<&str>) -> FileChange {
        FileChange {
            file: Pairing::RightOnly {
                rhs: FileRef {
                    path: "x".to_owned(),
                    oid: String::new(),
                    mode: String::new(),
                },
            },
            status: FileStatus::Added,
            category: category.map(str::to_owned),
            language: None,
            visibility: Visibility::default(),
        }
    }

    #[test]
    fn hidden_categories_follow_the_switches() {
        let rule = HiddenCategories {
            generated: true,
            tests: false,
        };
        let mut file = manifest(Some("generated"));
        rule.apply(&mut file).unwrap();
        assert!(file.visibility.collapsed);
        assert_eq!(file.visibility.label, "Generated file · hidden by default");
        let mut file = manifest(Some("test"));
        rule.apply(&mut file).unwrap();
        assert!(!file.visibility.collapsed);
        let mut file = manifest(None);
        rule.apply(&mut file).unwrap();
        assert!(!file.visibility.collapsed);
    }

    use crate::protocol::Span;

    fn removed_leaf(id: u32, start: u32, end: u32, changed: &[u32]) -> Region {
        Region {
            id,
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
            node: Node::Leaf {
                changed: changed
                    .iter()
                    .map(|&line| Span {
                        line,
                        start_column: 0,
                        end_column: 4,
                    })
                    .collect(),
            },
        }
    }

    fn left_only(regions: Vec<Region>) -> Pairing<Source> {
        Pairing::LeftOnly {
            lhs: Source {
                text: String::new(),
                syntax: vec![],
                regions,
            },
        }
    }

    fn shape(regions: &[Region]) -> Vec<(u32, u32, u32, bool, String, Vec<u32>)> {
        regions
            .iter()
            .map(|region| {
                let Node::Leaf { changed } = &region.node else {
                    panic!("leaf expected");
                };
                let lines = region.range.lines();
                (
                    region.id,
                    lines.start,
                    lines.end,
                    region.visibility.collapsed,
                    region.visibility.label.clone(),
                    changed.iter().map(|span| span.line).collect(),
                )
            })
            .collect()
    }

    #[test]
    fn removed_runs_keep_the_first_and_last_line_open() {
        let mut sides = left_only(vec![removed_leaf(0, 10, 17, &[10, 11, 12, 13, 14, 15, 16])]);
        RemovedRuns { min_lines: 5 }
            .apply(&manifest(None), &mut sides)
            .unwrap();
        assert_eq!(
            shape(&sides.lhs().unwrap().regions),
            vec![
                (0, 10, 11, false, String::new(), vec![10]),
                (
                    1,
                    11,
                    16,
                    true,
                    "5 lines removed".to_owned(),
                    vec![11, 12, 13, 14, 15]
                ),
                (2, 16, 17, false, String::new(), vec![16]),
            ]
        );
        let middle = &sides.lhs().unwrap().regions[1];
        assert_eq!(middle.tags, vec!["removed".to_owned()]);
    }

    #[test]
    fn removed_runs_respect_the_threshold_and_the_switch() {
        let mut sides = left_only(vec![removed_leaf(0, 0, 4, &[0, 1, 2, 3])]);
        RemovedRuns { min_lines: 5 }
            .apply(&manifest(None), &mut sides)
            .unwrap();
        assert_eq!(sides.lhs().unwrap().regions.len(), 1);
        // The registry never builds the rule at 0; a tiny threshold still
        // needs three lines to split.
        let mut sides = left_only(vec![removed_leaf(0, 0, 2, &[0, 1])]);
        RemovedRuns { min_lines: 1 }
            .apply(&manifest(None), &mut sides)
            .unwrap();
        assert_eq!(sides.lhs().unwrap().regions.len(), 1);
    }

    #[test]
    fn removed_runs_skip_paired_leaves_and_collapsed_ancestors() {
        let paired = removed_leaf(7, 0, 8, &[]);
        let mut sides = Pairing::Both {
            lhs: Source {
                text: String::new(),
                syntax: vec![],
                regions: vec![
                    paired.clone(),
                    Region {
                        id: 1,
                        range: SourceRange {
                            start: SourcePos { line: 8, column: 0 },
                            end: SourcePos {
                                line: 20,
                                column: 0,
                            },
                        },
                        tags: vec!["body".to_owned()],
                        visibility: Visibility {
                            collapsed: true,
                            label: "12 lines removed".to_owned(),
                        },
                        node: Node::Fold {
                            children: vec![removed_leaf(2, 8, 20, &[])],
                        },
                    },
                    removed_leaf(3, 20, 30, &[]),
                ],
            },
            rhs: Source {
                text: String::new(),
                syntax: vec![],
                regions: vec![paired],
            },
        };
        RemovedRuns { min_lines: 5 }
            .apply(&manifest(None), &mut sides)
            .unwrap();
        let lhs = &sides.lhs().unwrap().regions;
        assert_eq!(lhs.len(), 5, "paired leaf, collapsed fold, three pieces");
        assert_eq!(lhs[0].id, 7);
        let Node::Fold { children } = &lhs[1].node else {
            panic!("fold expected");
        };
        assert_eq!(
            children.len(),
            1,
            "leaf under a collapsed fold is untouched"
        );
        assert_eq!(
            shape(&lhs[2..]),
            vec![
                (3, 20, 21, false, String::new(), vec![]),
                (8, 21, 29, true, "8 lines removed".to_owned(), vec![]),
                (9, 29, 30, false, String::new(), vec![]),
            ]
        );
    }

    #[test]
    fn deleted_bodies_collapse_when_large_and_one_sided() {
        let before = "def gone():\n    a()\n    b()\n    c()\n\ndef kept():\n    a()\n    b()\n    c()\n\ndef tiny():\n    a()\n";
        let after = "def kept():\n    a()\n    b()\n    c()\n";
        let (file, mut sides) = crate::mutate::summarize::tests::project("m.py", before, after);
        DeletedBodies { min_lines: 3 }
            .apply(&file, &mut sides)
            .unwrap();
        let lhs = sides.lhs().unwrap();
        let mut collapsed = Vec::new();
        walk(&lhs.regions, &mut |region| {
            if is_fold(region) && region.visibility.collapsed {
                collapsed.push((region.range.start.line, region.visibility.label.clone()));
            }
        });
        assert_eq!(collapsed, vec![(1, "3 lines removed".to_owned())]);
    }
}
