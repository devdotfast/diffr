//! Fold metadata is attached during parsing; pairing reuses syntax identity.
use super::query::node_range;
use crate::config::query::AnnotationQuery;
use crate::diff::changes::ChangeKind;
use crate::hash::{DftHashMap, DftHashSet};
use crate::lines::{SourcePosition, SourceRange};
use crate::parse::syntax::{FoldMetadata, Syntax};
use streaming_iterator::StreamingIterator as _;
use tree_sitter::{QueryCursor, Tree};

#[derive(Debug)]
pub(crate) struct Fold {
    pub(crate) tags: Vec<String>,
    /// Source on this side; may span multiple syntax nodes.
    pub(crate) range: SourceRange,
    pub(crate) match_kind: FoldMatch,
    /// Text shown in place of the source, including supplied pseudocode.
    pub(crate) placeholder: String,
    /// Longer replacement text supplied by a configured fold hook.
    pub(crate) summary: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum FoldMatch {
    /// A corresponding fold, whose contents may differ. Collapsed state is client-owned.
    Unchanged {
        opposite: SourceRange,
    },
    Novel,
}

/// Interpret configurable fold captures in their own query traversal.
pub(crate) fn classify(
    tree: &Tree,
    src: &str,
    compiled: Option<&AnnotationQuery>,
) -> DftHashMap<usize, FoldMetadata> {
    let mut kinds = DftHashMap::default();
    let Some(compiled) = compiled else {
        return kinds;
    };
    let query = &compiled.query;
    let mut ambiguous_folds = DftHashSet::default();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), src.as_bytes());
    while let Some(matched) = matches.next() {
        let pattern = &compiled.patterns[matched.pattern_index];
        for fold in matched
            .captures
            .iter()
            .filter(|capture| query.capture_names()[capture.index as usize] == "fold")
        {
            let capture = |name| {
                matched
                    .captures
                    .iter()
                    .rev()
                    .find(|capture| query.capture_names()[capture.index as usize] == name)
            };
            let region = match (capture("fold.open"), capture("fold.close")) {
                (Some(open), Some(close))
                    if open.node.start_byte() >= fold.node.start_byte()
                        && close.node.end_byte() <= fold.node.end_byte()
                        && open.node.end_byte() <= close.node.start_byte() =>
                {
                    SourceRange {
                        start: node_range(open.node).end,
                        end: node_range(close.node).start,
                    }
                }
                (None, None) => node_range(fold.node),
                _ => continue,
            };
            if region.start == region.end || ambiguous_folds.contains(&fold.node.id()) {
                continue;
            }
            let metadata = kinds.entry(fold.node.id()).or_insert_with(|| FoldMetadata {
                tags: Vec::new(),
                range_override: Some(region),
            });
            if metadata.range_override != Some(region) {
                kinds.remove(&fold.node.id());
                ambiguous_folds.insert(fold.node.id());
                continue;
            }
            metadata.tags.extend(pattern.tags.iter().cloned());
            metadata.tags.sort();
            metadata.tags.dedup();
        }
    }
    kinds
}

/// Lists already retain the two edges of their interior, even without delimiters.
pub(crate) fn interior_range(
    open: &[line_numbers::SingleLineSpan],
    close: &[line_numbers::SingleLineSpan],
) -> SourceRange {
    let open = open.last().expect("list opening position");
    let close = close.first().expect("list closing position");
    SourceRange {
        start: SourcePosition {
            line: open.line,
            byte_column: open.end_col as usize,
        },
        end: SourcePosition {
            line: close.line,
            byte_column: close.start_col as usize,
        },
    }
}

fn range(node: &Syntax<'_>) -> Option<SourceRange> {
    let metadata = node.info().fold.borrow();
    let metadata = metadata.as_ref()?;
    let region = match (metadata.range_override, node) {
        (Some(region), _) => region,
        (
            None,
            Syntax::List {
                open_position,
                close_position,
                ..
            },
        ) => interior_range(open_position, close_position),
        (None, Syntax::Atom { position, .. }) => {
            let first = position.first().expect("atom start");
            let last = position.last().expect("atom end");
            SourceRange {
                start: SourcePosition {
                    line: first.line,
                    byte_column: first.start_col as usize,
                },
                end: SourcePosition {
                    line: last.line,
                    byte_column: last.end_col as usize,
                },
            }
        }
    };
    if (region.start.line, region.start.byte_column) >= (region.end.line, region.end.byte_column) {
        return None;
    }
    Some(region)
}

/// Project a side-local annotation using the same correspondence as MatchedPos.
pub(crate) fn project(node: &Syntax<'_>, change: ChangeKind<'_>) -> Option<Fold> {
    let own = node.info().fold.borrow().clone()?;
    let own_range = range(node)?;
    let opposite = match change {
        ChangeKind::Unchanged(other)
        | ChangeKind::ReplacedComment(_, other)
        | ChangeKind::ReplacedString(_, other) => range(other),
        _ => None,
    };
    let match_kind = match opposite {
        Some(opposite) => FoldMatch::Unchanged { opposite },
        None => FoldMatch::Novel,
    };
    Some(Fold {
        tags: own.tags.clone(),
        range: own_range,
        match_kind,
        placeholder: own
            .tags
            .first()
            .map(|tag| {
                let mut chars = tag.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                    .unwrap_or_default()
            })
            .unwrap_or_else(|| "…".into()),
        summary: None,
    })
}
