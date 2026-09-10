//! Source-coordinate helpers shared by the independent annotation queries.
use crate::lines::{SourcePosition, SourceRange};
use tree_sitter::Node;

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
