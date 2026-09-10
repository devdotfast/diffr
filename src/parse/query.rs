//! Source-coordinate helpers shared by the independent annotation queries.
use crate::config::query::Pattern;
use crate::lines::{SourcePosition, SourceRange};
use tree_sitter::{Node, QueryCapture};

pub(super) fn last_line(range: &SourceRange) -> usize {
    range.end.line.as_usize().saturating_sub(usize::from(
        range.end.byte_column == 0 && range.end.line > range.start.line,
    ))
}

pub(super) fn node_range(node: Node<'_>) -> SourceRange {
    let position = |point: tree_sitter::Point| SourcePosition {
        line: (point.row as u32).into(),
        byte_column: point.column,
    };
    SourceRange {
        start: position(node.start_position()),
        end: position(node.end_position()),
    }
}

pub(super) fn adjusted_range(
    capture: &QueryCapture<'_>,
    pattern: &Pattern,
    lines: &[&str],
) -> Option<SourceRange> {
    let mut range = node_range(capture.node);
    if let Some(offset) = pattern
        .offsets
        .iter()
        .find(|offset| offset.capture == capture.index)
    {
        let shift =
            |point: SourcePosition, rows: isize, columns: isize| -> Option<SourcePosition> {
                let row = point.line.as_usize().checked_add_signed(rows)?;
                let column = point.byte_column.checked_add_signed(columns)?;
                let line = lines.get(row)?;
                if column > line.len() || !line.is_char_boundary(column) {
                    return None;
                }
                Some(SourcePosition {
                    line: u32::try_from(row).ok()?.into(),
                    byte_column: column,
                })
            };
        range.start = shift(range.start, offset.start_row, offset.start_column)?;
        range.end = shift(range.end, offset.end_row, offset.end_column)?;
    }
    ((range.start.line, range.start.byte_column) <= (range.end.line, range.end.byte_column))
        .then_some(range)
}
