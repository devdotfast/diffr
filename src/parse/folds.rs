//! Fold metadata is attached during parsing; pairing reuses syntax identity.
use crate::diff::changes::ChangeKind;
use crate::hash::DftHashMap;
use crate::lines::{SourcePosition, SourceRange};
use crate::parse::syntax::Syntax;
use streaming_iterator::StreamingIterator as _;
use tree_sitter::{Query, QueryCursor, Tree};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub(crate) enum FoldKind {
    Import,
    Body,
    Collection,
    Test,
    Comment,
    String,
}

impl FoldKind {
    pub(crate) fn placeholder(self) -> &'static str {
        match self {
            Self::Import => "Import",
            Self::Body => "Body",
            Self::Collection => "Collection",
            Self::Test => "Test",
            Self::Comment => "Comment",
            Self::String => "String",
        }
    }
}

#[derive(Debug)]
pub(crate) struct Fold {
    pub(crate) kind: FoldKind,
    /// May span multiple syntax nodes. Paired regions share one fold toggle;
    /// current collapsed state belongs to the client.
    pub(crate) regions: Correspondence<SourceRange>,
    /// Text shown in place of the source, including supplied pseudocode.
    pub(crate) placeholder: String,
}

#[derive(Debug, Clone)]
pub(crate) enum Correspondence<T> {
    /// Corresponding regions need not contain identical text.
    Paired {
        lhs: T,
        rhs: T,
    },
    Deleted(T),
    Added(T),
}

pub(crate) fn classify(
    tree: &Tree,
    src: &str,
    query: Option<&Query>,
) -> DftHashMap<usize, FoldKind> {
    let mut kinds = DftHashMap::default();
    let Some(query) = query else {
        return kinds;
    };
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, tree.root_node(), src.as_bytes());
    while let Some(matched) = matches.next() {
        for capture in matched.captures {
            let kind = match query.capture_names()[capture.index as usize] {
                "fold.body" => FoldKind::Body,
                "fold.collection" => FoldKind::Collection,
                "fold.import" => FoldKind::Import,
                "fold.test" => FoldKind::Test,
                "fold.comment" => FoldKind::Comment,
                "fold.string" => FoldKind::String,
                name if name == "name" || name == "attribute" || name.starts_with("context.") => {
                    continue
                }
                name => panic!("unknown fold capture: {name}"),
            };
            // Test is more specific than the generic body capture.
            if kinds.get(&capture.node.id()) != Some(&FoldKind::Test) {
                kinds.insert(capture.node.id(), kind);
            }
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
    let metadata = node.info().fold.get()?;
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

/// Project the same node match used for MatchedPos. Paired folds emit on the left only.
pub(crate) fn project(
    node: &Syntax<'_>,
    change: ChangeKind<'_>,
    side: crate::constants::Side,
) -> Option<Fold> {
    use crate::constants::Side;
    let own = node.info().fold.get()?;
    let own_range = range(node)?;
    let opposite = match change {
        ChangeKind::Unchanged(other)
        | ChangeKind::ReplacedComment(_, other)
        | ChangeKind::ReplacedString(_, other) => range(other),
        _ => None,
    };
    let regions = match (side, opposite) {
        (Side::Left, Some(other)) => Correspondence::Paired {
            lhs: own_range,
            rhs: other,
        },
        (Side::Right, Some(_)) => return None,
        (Side::Left, None) => Correspondence::Deleted(own_range),
        (Side::Right, None) => Correspondence::Added(own_range),
    };
    Some(Fold {
        kind: own.kind,
        regions,
        placeholder: own.kind.placeholder().into(),
    })
}
