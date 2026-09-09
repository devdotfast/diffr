//! Semantic context boundaries captured before Tree-sitter wrappers are flattened.
use crate::hash::DftHashMap;
use streaming_iterator::StreamingIterator as _;
use tree_sitter::{Query, QueryCursor, Tree};

/// Whole-line context, independent of the eventual hunk or display layout.
#[derive(Clone, Debug)]
pub(crate) struct ContextMetadata {
    pub(crate) contains: std::ops::RangeInclusive<usize>,
    pub(crate) header: std::ops::RangeInclusive<usize>,
    pub(crate) closing: Option<usize>,
}

pub(crate) fn classify(
    tree: &Tree,
    src: &str,
    query: Option<&Query>,
) -> DftHashMap<usize, Vec<ContextMetadata>> {
    let mut contexts = DftHashMap::<_, Vec<_>>::default();
    let Some(query) = query else {
        return contexts;
    };
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), src.as_bytes());
    while let Some(matched) = matches.next() {
        let capture = |name| {
            matched
                .captures
                .iter()
                .find(|capture| query.capture_names()[capture.index as usize] == name)
                .map(|capture| capture.node)
        };
        let Some(owner) = capture("context.scope").or_else(|| capture("context.boundary")) else {
            continue;
        };
        let start = owner.start_position().row;
        let end = owner.end_position().row;
        let (header_end, closing) = match capture("context.body") {
            Some(body) => (
                body.start_position().row,
                capture("context.close").map(|node| node.start_position().row),
            ),
            None => match capture("context.indented_body") {
                Some(body) => (body.start_position().row.saturating_sub(1), None),
                None => (start, Some(end)),
            },
        };
        contexts
            .entry(owner.id())
            .or_default()
            .push(ContextMetadata {
                contains: start..=end,
                header: start..=header_end,
                closing,
            });
    }
    contexts
}
