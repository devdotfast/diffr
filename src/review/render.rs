//! Deterministic, uncolored review snapshots. Fold candidates do not hide source.
use std::fmt::Write;

use crate::display::line_layout::{reindented_pairs, sources, LineSelection};
use crate::summary::DiffResult;

impl DiffResult {
    pub(crate) fn snapshot(&self) -> String {
        snapshot(self)
    }
}

pub(super) fn snapshot(diff: &DiffResult) -> String {
    let (lhs_src, rhs_src) = sources(diff);
    let lhs_lines: Vec<_> = lhs_src.split_terminator('\n').collect();
    let rhs_lines: Vec<_> = rhs_src.split_terminator('\n').collect();
    let selected = LineSelection::from_hunks(&diff.hunks);
    let mut out = format!("--- a/{}\n+++ b/{}\n", diff.display_path, diff.display_path);
    if diff.hunks.is_empty() {
        out.push_str("(no syntactic changes)\n");
        return out;
    }
    let reindented = reindented_pairs(diff);
    let mut writer = SnapshotWriter::new(out);
    for (l, r) in crate::display::line_layout::aligned_rows(
        (lhs_src, rhs_src),
        (&diff.lhs_positions, &diff.rhs_positions),
    ) {
        let left = l.filter(|line| selected.lhs.contains(line));
        let right = r.filter(|line| selected.rhs.contains(line));
        if left.is_none() && right.is_none() {
            writer.flush_changes();
            writer.gap = true;
            continue;
        }
        writer.write_gap();
        match (left, right) {
            (Some(l), Some(r)) if lhs_lines[l] == rhs_lines[r] => {
                writer.flush_changes();
                writeln!(
                    writer.output,
                    "{:>4} {:>4}   {}",
                    l + 1,
                    r + 1,
                    lhs_lines[l]
                )
                .unwrap();
            }
            (Some(l), Some(r)) if reindented.contains(&(l, r)) => {
                writer.flush_changes();
                writeln!(writer.output, "{:>4}      ~ {}", l + 1, lhs_lines[l]).unwrap();
                writeln!(writer.output, "     {:>4} ~ {}", r + 1, rhs_lines[r]).unwrap();
            }
            (l, r) => {
                if let Some(l) = l {
                    writeln!(writer.removed, "{:>4}      - {}", l + 1, lhs_lines[l]).unwrap();
                }
                if let Some(r) = r {
                    writeln!(writer.added, "     {:>4} + {}", r + 1, rhs_lines[r]).unwrap();
                }
            }
        }
    }
    writer.flush_changes();
    writer.write_gap();
    for (side, source) in [("base", lhs_src), ("head", rhs_src)] {
        if !source.is_empty() && !source.ends_with('\n') {
            writeln!(writer.output, "\\ No newline at end of {} file", side).unwrap();
        }
    }
    writer.output
}

/// Buffer deletions before additions within a contiguous change block.
struct SnapshotWriter {
    output: String,
    removed: String,
    added: String,
    gap: bool,
}

impl SnapshotWriter {
    fn new(output: String) -> Self {
        Self {
            output,
            removed: String::new(),
            added: String::new(),
            gap: false,
        }
    }

    fn flush_changes(&mut self) {
        self.output.push_str(&self.removed);
        self.output.push_str(&self.added);
        self.removed.clear();
        self.added.clear();
    }

    fn write_gap(&mut self) {
        if !self.gap {
            return;
        }
        self.output.push_str("          | ...\n");
        self.gap = false;
    }
}

#[cfg(test)]
mod tests {
    use crate::parse::syntax::MatchKind;
    use crate::summary::DiffResult;
    #[test]
    fn matched_rename_to_is_reindented_not_deleted_and_added() {
        let lhs = include_str!("../../examples/review/real/02-review-175/before.ts");
        let rhs = include_str!("../../examples/review/real/02-review-175/after.ts");
        let review = DiffResult::from_sources("parser.ts", lhs, rhs);
        assert!(super::reindented_pairs(&review).contains(&(247, 253)));
        let output = review.snapshot();
        let assignment: Vec<_> = output
            .lines()
            .filter(|line| line.contains("renameTo = unquoteGitPath"))
            .collect();
        assert_eq!(assignment.len(), 2);
        assert!(assignment.iter().all(|line| line.as_bytes()[10] == b'~'));
        let token = review
            .lhs_positions
            .iter()
            .find(|p| p.pos.line.0 == 247 && p.pos.start_col == 6)
            .unwrap();
        let MatchKind::UnchangedToken { opposite_pos, .. } = &token.kind else {
            panic!(
                "the renamed binding is an unchanged token: {:?}",
                token.kind
            );
        };
        assert_eq!(
            (opposite_pos[0].line.0, opposite_pos[0].start_col),
            (253, 8)
        );
        assert!(review.lhs_folds[0].counterpart(&review.rhs_folds).is_some());
    }

    #[test]
    fn changed_literal_is_not_treated_as_reindentation() {
        let review = DiffResult::from_sources(
            "a.py",
            "def f():\n    return ' a'\n",
            "def f():\n    return '  a'\n",
        );
        assert!(!super::reindented_pairs(&review).contains(&(1, 1)));
    }
}
