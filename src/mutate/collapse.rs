//! Rules that start things collapsed: whole files by category, deleted
//! function bodies, test bodies, and the middle of large removed stretches.
use super::group::next_id;
use super::{collapse, ids, is_fold, line_count, one_sided, walk_mut, FileMutation, FoldMutation};
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

/// Deleted function bodies of at least `min_lines` start collapsed with a
/// line count. The header line stays visible by fold semantics. A body
/// counts as deleted only when nothing under it is paired: a function
/// whose header moved but whose lines still align is a rewrite and stays
/// open.
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
                && region.tags.iter().any(|tag| tag == "function")
                && one_sided(region, &rhs_ids)
                && line_count(region) >= self.min_lines
            {
                let count = line_count(region);
                collapse(region, format!("{count} lines removed"));
            }
        });
        Ok(())
    }
}

/// Test bodies start collapsed on both sides, paired or not, so a diff
/// reads as the code under test first. A whole test module, such as a
/// Rust `#[cfg(test)] mod tests`, collapses as one fold labelled
/// "test module". The header stays visible and the fold expands like any
/// other.
pub(crate) struct TestBodies;

/// A test body shorter than this stays open: a one-line assertion is
/// cheaper to read than a fold row.
const MIN_TEST_BODY_LINES: usize = 3;

impl FoldMutation for TestBodies {
    fn apply(&self, _file: &FileChange, sides: &mut Pairing<Source>) -> Result<(), Problem> {
        let sources: Vec<&mut Source> = match sides {
            Pairing::Both { lhs, rhs } => vec![lhs, rhs],
            Pairing::LeftOnly { lhs } => vec![lhs],
            Pairing::RightOnly { rhs } => vec![rhs],
        };
        for source in sources {
            walk_mut(&mut source.regions, &mut |region| {
                if is_fold(region)
                    && region.tags.iter().any(|tag| tag == "test")
                    && line_count(region) >= MIN_TEST_BODY_LINES
                {
                    let label = if region.tags.iter().any(|tag| tag == "module") {
                        "test module"
                    } else {
                        "test body"
                    };
                    collapse(region, label.to_owned());
                }
            });
        }
        Ok(())
    }
}

/// Removed stretches with no counterpart and at least `min_lines` lines are
/// split into three leaves: the first line open, the middle collapsed with
/// a line count, the last line open, so the reader still sees red at both
/// ends. Only stretches in unpaired code qualify: the nearest enclosing
/// `function` fold (or, outside any function, the nearest enclosing fold)
/// must itself be one-sided, so a rewritten function shows its red and
/// green lines in place. Stretches at the top level keep the plain rule.
/// Leaves under a fold that already starts collapsed are left alone.
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
        split_removed_runs(
            &mut lhs.regions,
            &rhs_ids,
            threshold,
            false,
            Gates::default(),
            &mut next_id,
        );
        Ok(())
    }
}

/// Whether the enclosing folds are one-sided: the nearest `function` fold,
/// and the nearest fold of any tag. `None` when there is no such fold.
#[derive(Clone, Copy, Default)]
struct Gates {
    function: Option<bool>,
    any: Option<bool>,
}

impl Gates {
    fn enter(self, fold: &Region, rhs_ids: &DftHashSet<u32>) -> Self {
        let unpaired = one_sided(fold, rhs_ids);
        Self {
            function: if fold.tags.iter().any(|tag| tag == "function") {
                Some(unpaired)
            } else {
                self.function
            },
            any: Some(unpaired),
        }
    }

    /// Inside a paired function nothing collapses; outside any function
    /// the nearest fold decides; at the top level everything qualifies.
    fn open(self) -> bool {
        self.function.or(self.any).unwrap_or(true)
    }
}

fn split_removed_runs(
    regions: &mut Vec<Region>,
    rhs_ids: &DftHashSet<u32>,
    threshold: usize,
    under_collapsed: bool,
    gates: Gates,
    next_id: &mut u32,
) {
    let mut out = Vec::with_capacity(regions.len());
    for mut region in regions.drain(..) {
        let collapsed = under_collapsed || region.visibility.collapsed;
        let splits = !under_collapsed
            && gates.open()
            && !rhs_ids.contains(&region.alignment_id)
            && line_count(&region) >= threshold;
        let inner = if is_fold(&region) {
            gates.enter(&region, rhs_ids)
        } else {
            gates
        };
        match &mut region.node {
            Node::Fold { children } => {
                split_removed_runs(children, rhs_ids, threshold, collapsed, inner, next_id);
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
            alignment_id: id,
            fold_state_id: id,
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
            region.alignment_id,
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

    #[test]
    fn test_bodies_collapse_on_both_sides_and_stay_expandable() {
        let before = "#[test]\nfn t() {\n    a();\n    b();\n    c();\n}\n\nfn f() {\n    a();\n    b();\n    c();\n}\n";
        let after = "#[test]\nfn t() {\n    a();\n    b();\n    changed();\n}\n\nfn f() {\n    a();\n    b();\n    c();\n}\n";
        let (file, mut sides) = crate::mutate::summarize::tests::project("a.rs", before, after);
        TestBodies.apply(&file, &mut sides).unwrap();
        for source in [sides.lhs().unwrap(), sides.rhs().unwrap()] {
            let mut folds = Vec::new();
            walk(&source.regions, &mut |region| {
                if is_fold(region) {
                    folds.push((
                        region.tags.contains(&"test".to_owned()),
                        region.visibility.collapsed,
                        region.visibility.label.clone(),
                    ));
                }
            });
            assert_eq!(
                folds,
                vec![
                    (true, true, "test body".to_owned()),
                    (false, false, "Body".to_owned())
                ]
            );
        }
        // A `#[cfg(test)]` module collapses as one labelled fold; the test
        // bodies inside keep their own label.
        let (file, mut sides) = crate::mutate::summarize::tests::project(
            "a.rs",
            "",
            "#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        a();\n        b();\n        c();\n    }\n}\n",
        );
        TestBodies.apply(&file, &mut sides).unwrap();
        let mut labels = Vec::new();
        walk(&sides.rhs().unwrap().regions, &mut |region| {
            if is_fold(region) && region.visibility.collapsed {
                labels.push(region.visibility.label.clone());
            }
        });
        assert_eq!(labels, ["test module", "test body"]);
        // A tiny test body stays open.
        let (file, mut sides) = crate::mutate::summarize::tests::project(
            "a.rs",
            "",
            "#[test]\nfn t() {\n    a();\n}\n",
        );
        TestBodies.apply(&file, &mut sides).unwrap();
        walk(&sides.rhs().unwrap().regions, &mut |region| {
            assert!(!region.visibility.collapsed);
        });
    }
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
                    region.alignment_id,
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
                        alignment_id: 1,
                        fold_state_id: 1,
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
        assert_eq!(lhs[0].alignment_id, 7);
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

    fn function_fold(id: u32, start: u32, end: u32, children: Vec<Region>) -> Region {
        Region {
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
            visibility: Visibility::default(),
            node: Node::Fold { children },
        }
    }

    fn both(lhs: Vec<Region>, rhs: Vec<Region>) -> Pairing<Source> {
        let source = |regions| Source {
            text: String::new(),
            syntax: vec![],
            regions,
        };
        Pairing::Both {
            lhs: source(lhs),
            rhs: source(rhs),
        }
    }

    #[test]
    fn removed_runs_stay_open_under_a_paired_function() {
        // The lhs function fold has no counterpart, but its second leaf
        // does: a rewrite, so its removed stretch stays in place.
        let rewritten = function_fold(
            1,
            0,
            20,
            vec![
                removed_leaf(2, 0, 10, &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9]),
                removed_leaf(3, 10, 20, &[]),
            ],
        );
        // Wholly removed: nothing under it is on the rhs.
        let removed = function_fold(
            4,
            20,
            40,
            vec![removed_leaf(5, 20, 40, &[20, 21, 22, 23, 24, 25, 26, 27])],
        );
        let mut sides = both(vec![rewritten, removed], vec![removed_leaf(3, 0, 10, &[])]);
        RemovedRuns { min_lines: 5 }
            .apply(&manifest(None), &mut sides)
            .unwrap();
        let lhs = &sides.lhs().unwrap().regions;
        let Node::Fold { children } = &lhs[0].node else {
            panic!("fold expected");
        };
        assert_eq!(children.len(), 2, "nothing split under the paired function");
        assert!(children.iter().all(|child| !child.visibility.collapsed));
        let Node::Fold { children } = &lhs[1].node else {
            panic!("fold expected");
        };
        assert_eq!(
            shape(children),
            vec![
                (5, 20, 21, false, String::new(), vec![20]),
                (
                    6,
                    21,
                    39,
                    true,
                    "18 lines removed".to_owned(),
                    (21..28).collect()
                ),
                (7, 39, 40, false, String::new(), vec![]),
            ]
        );
    }

    #[test]
    fn deleted_bodies_skip_folds_with_paired_content() {
        // Header moved (fold id absent on the rhs) but the body lines align.
        let rewritten = function_fold(1, 0, 20, vec![removed_leaf(2, 0, 20, &[])]);
        let removed = function_fold(3, 20, 40, vec![removed_leaf(4, 20, 40, &[])]);
        let mut sides = both(vec![rewritten, removed], vec![removed_leaf(2, 0, 20, &[])]);
        DeletedBodies { min_lines: 3 }
            .apply(&manifest(None), &mut sides)
            .unwrap();
        let lhs = &sides.lhs().unwrap().regions;
        assert!(!lhs[0].visibility.collapsed, "a rewrite stays open");
        assert!(lhs[1].visibility.collapsed);
        assert_eq!(lhs[1].visibility.label, "20 lines removed");
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
