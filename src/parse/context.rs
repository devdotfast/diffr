//! Semantic context boundaries captured before Tree-sitter wrappers are flattened.

/// Whole-line context, independent of the eventual hunk or display layout.
#[derive(Clone, Debug)]
pub(crate) struct ContextMetadata {
    pub(crate) contains: std::ops::RangeInclusive<usize>,
    pub(crate) header: std::ops::RangeInclusive<usize>,
    pub(crate) closing: Option<usize>,
}
