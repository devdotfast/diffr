//! What a diff looks like once the plugins have shaped it: the changed
//! lines that stay visible.
use crate::hash::DftHashSet;
use crate::pairing::Pairing;
use crate::protocol::{Diff, LineCounts, LineRange, Node, Region, Source, Visibility};

/// Run the plugins on a diff and recount what stays visible. `shape` runs
/// the plugins on a text diff's sides. A hidden file runs no plugin: its
/// roots start collapsed behind the reason. A binary diff has nothing to
/// shape. `Err` is a run-level failure.
pub async fn present(
    hidden: Option<&str>,
    diff: Diff,
    shape: impl AsyncFnOnce(Pairing<Source>) -> anyhow::Result<Pairing<Source>>,
) -> anyhow::Result<Diff> {
    match diff {
        Diff::Text {
            sides, mut stats, ..
        } => {
            let sides = match hidden {
                Some(reason) => sides.map(|mut source| {
                    source.root.visibility = Visibility {
                        collapsed: true,
                        label: reason.to_owned(),
                    };
                    source
                }),
                None => shape(sides).await?,
            };
            stats.visible = visible_changes(&sides);
            Ok(Diff::Text { sides, stats })
        }
        Diff::Binary { sides } => Ok(Diff::Binary { sides }),
    }
}

/// Count the changed lines that start visible. A paired leaf counts only
/// lines carrying changed spans; every line of an unpaired leaf counts,
/// including blank lines.
fn visible_changes(sides: &Pairing<Source>) -> LineCounts {
    fn alignments(regions: &[Region], out: &mut DftHashSet<u32>) {
        for region in regions {
            match &region.node {
                Node::Leaf { alignment_id, .. } => {
                    out.insert(*alignment_id);
                }
                Node::Fold { children, .. } => alignments(children, out),
            }
        }
    }
    fn collect(
        regions: &[Region],
        other: &DftHashSet<u32>,
        hidden: bool,
        visible: &mut Vec<LineRange>,
    ) {
        for region in regions {
            let hidden = hidden || region.visibility.collapsed;
            match &region.node {
                Node::Leaf { .. } if hidden => {}
                Node::Leaf {
                    alignment_id,
                    changed,
                } => {
                    if other.contains(alignment_id) {
                        visible.extend(changed.iter().map(|span| [span.line, span.line + 1]));
                    } else {
                        let lines = region.range.lines();
                        visible.push([lines.start, lines.end]);
                    }
                }
                Node::Fold { children, .. } => collect(children, other, hidden, visible),
            }
        }
    }
    fn side(source: Option<&Source>, other: Option<&Source>) -> u32 {
        let mut paired = DftHashSet::default();
        if let Some(other) = other {
            alignments(std::slice::from_ref(&other.root), &mut paired);
        }
        let mut visible = Vec::new();
        if let Some(source) = source {
            collect(
                std::slice::from_ref(&source.root),
                &paired,
                false,
                &mut visible,
            );
        }
        coalesce(visible)
            .iter()
            .map(|[start, end]| end - start)
            .sum()
    }
    let (lhs, rhs) = match sides {
        Pairing::Both { lhs, rhs } => (Some(lhs), Some(rhs)),
        Pairing::LeftOnly { lhs } => (Some(lhs), None),
        Pairing::RightOnly { rhs } => (None, Some(rhs)),
    };
    LineCounts {
        added: side(rhs, lhs),
        removed: side(lhs, rhs),
    }
}

/// Compact spans and whole-leaf ranges without allocating one entry per source line.
fn coalesce(mut ranges: Vec<LineRange>) -> Vec<LineRange> {
    ranges.sort_unstable();
    let mut merged: Vec<LineRange> = Vec::new();
    for [start, end] in ranges {
        if start >= end {
            continue;
        }
        if let Some(last) = merged.last_mut() {
            if start <= last[1] {
                last[1] = last[1].max(end);
                continue;
            }
        }
        merged.push([start, end]);
    }
    merged
}

#[cfg(test)]
mod visible_tests {
    use super::*;
    use crate::protocol::{SourcePos, SourceRange, Span};

    fn test_root(regions: Vec<Region>) -> Region {
        let id = 1000 + regions.iter().map(|region| region.id).min().unwrap_or(0);
        Region::root(id, regions)
    }

    fn pos(line: u32) -> SourcePos {
        SourcePos { line, column: 0 }
    }

    fn leaf(
        id: u32,
        alignment: u32,
        lines: (u32, u32),
        changed: &[u32],
        collapsed: bool,
    ) -> Region {
        Region {
            id,
            fold_state_id: id,
            range: SourceRange {
                start: pos(lines.0),
                end: pos(lines.1),
            },
            tags: vec![],
            visibility: Visibility {
                collapsed,
                label: String::new(),
            },
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

    fn fold(id: u32, lines: (u32, u32), collapsed: bool, children: Vec<Region>) -> Region {
        Region {
            id,
            fold_state_id: id,
            range: SourceRange {
                start: pos(lines.0),
                end: pos(lines.1),
            },
            tags: vec![],
            visibility: Visibility {
                collapsed,
                label: String::new(),
            },
            node: Node::Fold {
                indent: pos(lines.0),
                syntax: None,
                children,
            },
        }
    }

    fn source(regions: Vec<Region>) -> Source {
        Source {
            text: String::new(),
            syntax: vec![],
            root: test_root(regions),
        }
    }

    #[test]
    fn counts_span_lines_and_every_line_of_a_one_sided_leaf() {
        let rhs = source(vec![
            // paired leaf: only the lines with spans count (two, one twice)
            leaf(0, 1, (0, 3), &[0, 1, 1], false),
            // paired leaf without spans: unchanged context, not counted
            leaf(1, 9, (3, 4), &[], false),
            // one-sided leaf with no spans (blank lines): every line counts
            leaf(2, 2, (4, 6), &[], false),
            // collapsed leaf: hidden
            leaf(3, 3, (6, 9), &[6, 7], true),
            // open fold with an open one-sided leaf: every line counts
            fold(4, (9, 12), false, vec![leaf(5, 5, (9, 12), &[10], false)]),
            // collapsed fold: its open child is hidden by the ancestor
            fold(
                6,
                (12, 15),
                true,
                vec![leaf(7, 7, (12, 15), &[13, 14], false)],
            ),
        ]);
        let lhs = source(vec![
            leaf(8, 1, (0, 3), &[0], false),
            leaf(9, 9, (3, 4), &[], false),
            leaf(10, 8, (4, 7), &[4, 5], true),
        ]);
        let counts = visible_changes(&Pairing::Both { lhs, rhs });
        assert_eq!(counts.added, 2 + 2 + 3);
        assert_eq!(counts.removed, 1);
    }

    #[test]
    fn every_line_of_a_deleted_file_counts_even_blank_ones() {
        let lhs = source(vec![leaf(1, 0, (0, 2), &[], false)]);
        assert_eq!(visible_changes(&Pairing::LeftOnly { lhs }).removed, 2);
    }
}
