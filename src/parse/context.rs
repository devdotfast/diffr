//! Semantic context boundaries captured before Tree-sitter wrappers are flattened.

/// Whole-line context, independent of the eventual hunk or display layout.
#[derive(Clone, Debug)]
pub(crate) struct ContextMetadata {
    pub(crate) contains: std::ops::RangeInclusive<usize>,
    pub(crate) header: std::ops::RangeInclusive<usize>,
    pub(crate) closing: Option<usize>,
}

/// Interpret configurable context captures in their own query traversal.
pub(crate) fn classify(
    tree: &tree_sitter::Tree,
    src: &str,
    compiled: Option<&crate::config::query::AnnotationQuery>,
) -> crate::hash::DftHashMap<usize, Vec<ContextMetadata>> {
    use super::query::{adjusted_range, last_line, node_range};
    use crate::{hash::DftHashMap, lines::SourcePosition};
    use streaming_iterator::StreamingIterator as _;
    let mut result: DftHashMap<usize, Vec<ContextMetadata>> = DftHashMap::default();
    let Some(compiled) = compiled else {
        return result;
    };
    let query = &compiled.query;
    let lines: Vec<_> = src.split('\n').collect();
    let mut cursor = tree_sitter::QueryCursor::new();
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
