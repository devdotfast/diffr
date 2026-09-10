//! Evaluate one compiled query per syntax tree, retaining metadata before flattening.
use super::{context::ContextMetadata, syntax::FoldMetadata};
use crate::config::query::{AnnotationQuery, Pattern};
use crate::hash::{DftHashMap, DftHashSet};
use crate::lines::{SourcePosition, SourceRange};
use streaming_iterator::StreamingIterator as _;
use tree_sitter::{Node, QueryCapture, QueryCursor};

#[derive(Default)]
pub(crate) struct Annotations {
    pub(crate) folds: DftHashMap<usize, FoldMetadata>,
    pub(crate) contexts: DftHashMap<usize, Vec<ContextMetadata>>,
}

pub(crate) fn collect(
    tree: &tree_sitter::Tree,
    src: &str,
    compiled: Option<&AnnotationQuery>,
) -> Annotations {
    let mut result = Annotations::default();
    let Some(compiled) = compiled else {
        return result;
    };
    let query = &compiled.query;
    let lines: Vec<_> = src.split('\n').collect();
    let mut ambiguous_folds = DftHashSet::default();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), src.as_bytes());
    while let Some(matched) = matches.next() {
        let pattern = &compiled.patterns[matched.pattern_index];
        let capture = |name: &str| {
            matched
                .captures
                .iter()
                .rev()
                .find(|capture| query.capture_names()[capture.index as usize] == name)
        };
        for fold in matched
            .captures
            .iter()
            .filter(|capture| query.capture_names()[capture.index as usize] == "fold")
        {
            let Some(region) = adjusted_range(fold, pattern, &lines) else {
                continue;
            };
            if region.start == region.end || ambiguous_folds.contains(&fold.node.id()) {
                continue;
            }
            let metadata = result
                .folds
                .entry(fold.node.id())
                .or_insert_with(|| FoldMetadata {
                    tags: Vec::new(),
                    range_override: Some(region),
                });
            if metadata.range_override != Some(region) {
                result.folds.remove(&fold.node.id());
                ambiguous_folds.insert(fold.node.id());
                continue;
            }
            metadata.tags.extend(pattern.tags.iter().cloned());
            metadata.tags.sort();
            metadata.tags.dedup();
        }
        let Some(owner) = capture("context") else {
            continue;
        };
        let scope = node_range(owner.node);
        let start = capture("context.start")
            .and_then(|capture| adjusted_range(capture, pattern, &lines))
            .map_or(scope.start, |range| range.start);
        let end = if let Some(capture) = capture("context.end") {
            let Some(range) = adjusted_range(capture, pattern, &lines) else {
                continue;
            };
            range.start
        } else if let Some(capture) = capture("context.final") {
            let Some(range) = adjusted_range(capture, pattern, &lines) else {
                continue;
            };
            range.end
        } else {
            SourcePosition {
                line: (scope.start.line.0 + 1).into(),
                byte_column: 0,
            }
        };
        if (start.line, start.byte_column) >= (end.line, end.byte_column) {
            continue;
        }
        let last = capture("context.last")
            .and_then(|capture| adjusted_range(capture, pattern, &lines))
            .map(|range| last_line(&range));
        result
            .contexts
            .entry(owner.node.id())
            .or_default()
            .push(ContextMetadata {
                contains: scope.start.line.as_usize()..=last_line(&scope),
                header: start.line.as_usize()
                    ..=end
                        .line
                        .as_usize()
                        .saturating_sub(usize::from(end.byte_column == 0)),
                closing: last,
            });
    }
    result
}

fn last_line(range: &SourceRange) -> usize {
    range.end.line.as_usize().saturating_sub(usize::from(
        range.end.byte_column == 0 && range.end.line > range.start.line,
    ))
}

fn node_range(node: Node<'_>) -> SourceRange {
    let position = |point: tree_sitter::Point| SourcePosition {
        line: (point.row as u32).into(),
        byte_column: point.column,
    };
    SourceRange {
        start: position(node.start_position()),
        end: position(node.end_position()),
    }
}

fn adjusted_range(
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
