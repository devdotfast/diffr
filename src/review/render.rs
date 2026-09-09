//! Deterministic, uncolored review snapshots. Fold candidates do not hide source.
use std::fmt::Write;

use crate::display::line_layout::{aligned_rows, baseline, reindented_pairs, sources};
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
    let positions = (&diff.lhs_positions[..], &diff.rhs_positions[..]);
    let rows = aligned_rows((lhs_src, rhs_src), positions);
    let mut selected = baseline(positions, &rows);
    for hunk in &diff.hunks {
        selected.include_context(&hunk.context);
    }
    let mut out = format!("--- a/{}\n+++ b/{}\n", diff.display_path, diff.display_path);
    if selected.lhs.is_empty() && selected.rhs.is_empty() {
        out.push_str("(no syntactic changes)\n");
        return out;
    }
    let reindented = reindented_pairs(diff);
    let mut writer = SnapshotWriter::new(out);
    for (l, r) in rows {
        let left = l.filter(|l| selected.lhs.contains(l));
        let right = r.filter(|r| selected.rhs.contains(r));
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

