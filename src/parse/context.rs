//! Semantic context boundaries captured before Tree-sitter wrappers are flattened.
use super::query::{last_line, node_range};
use crate::config::query::AnnotationQuery;
use crate::hash::DftHashMap;
use crate::lines::SourcePosition;
use streaming_iterator::StreamingIterator as _;
use tree_sitter::{QueryCursor, Tree};

/// Whole-line context, independent of the eventual hunk or display layout.
#[derive(Clone, Debug)]
pub(crate) struct ContextMetadata {
    pub(crate) contains: std::ops::RangeInclusive<usize>,
    pub(crate) header: std::ops::RangeInclusive<usize>,
    pub(crate) closing: Option<usize>,
}

/// Interpret configurable context captures in their own query traversal.
pub(crate) fn classify(
    tree: &Tree,
    src: &str,
    compiled: Option<&AnnotationQuery>,
) -> DftHashMap<usize, Vec<ContextMetadata>> {
    let mut contexts = DftHashMap::<_, Vec<_>>::default();
    let Some(compiled) = compiled else {
        return contexts;
    };
    let query = &compiled.query;
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), src.as_bytes());
    while let Some(matched) = matches.next() {
        let capture = |name| {
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
        let start =
            capture("context.start").map_or(scope.start, |capture| node_range(capture.node).start);
        let end = if let Some(capture) = capture("context.end") {
            node_range(capture.node).start
        } else if let Some(capture) = capture("context.final") {
            node_range(capture.node).end
        } else {
            SourcePosition {
                line: (scope.start.line.0 + 1).into(),
                byte_column: 0,
            }
        };
        if (start.line, start.byte_column) >= (end.line, end.byte_column) {
            continue;
        }
        let last = capture("context.last").map(|capture| last_line(&node_range(capture.node)));
        contexts
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
    contexts
}
