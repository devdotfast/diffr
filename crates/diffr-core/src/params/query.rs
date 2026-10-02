//! Compile the supported Tree-sitter capture and directive conventions.
//!
//! A query is compiled from one or more named sources, concatenated in
//! order. Every pattern remembers which source it came from, so errors and
//! conflicts can name the file that wrote it.
use tree_sitter::Query;

/// A fold query that does not compile, or breaks a convention; the message
/// names the source that wrote it.
#[derive(Debug)]
pub struct QueryError(pub String);

impl std::fmt::Display for QueryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for QueryError {}

pub struct AnnotationQuery {
    pub query: Query,
    pub patterns: Vec<Pattern>,
    /// The name of every source, indexed by `Pattern::source`.
    pub sources: Vec<String>,
}

pub struct Pattern {
    pub tags: Vec<String>,
    /// Index into `AnnotationQuery::sources`.
    pub source: usize,
}

/// One piece of query text and the name errors call it by: a query file's
/// path, or a configuration key.
pub struct QuerySource {
    pub name: String,
    pub text: String,
}

impl AnnotationQuery {
    pub fn compile(
        grammar: &tree_sitter::Language,
        sources: &[QuerySource],
    ) -> Result<Self, QueryError> {
        let mut text = String::new();
        let mut starts = Vec::with_capacity(sources.len());
        for source in sources {
            starts.push(text.len());
            text.push_str(&source.text);
            text.push('\n');
        }
        // The source a byte of the concatenated text belongs to.
        let owner = |offset: usize| starts.partition_point(|&start| start <= offset) - 1;
        let query = Query::new(grammar, &text).map_err(|error| {
            if sources.is_empty() {
                return QueryError(error.to_string());
            }
            let index = owner(error.offset.min(text.len().saturating_sub(1)));
            let line = text[starts[index]..error.offset].matches('\n').count() + 1;
            QueryError(format!(
                "{}:{line}: {:?} error: {}",
                sources[index].name, error.kind, error.message
            ))
        })?;
        let named = |pattern: usize, message: String| {
            QueryError(format!(
                "{}: {message}",
                sources[owner(query.start_byte_for_pattern(pattern))].name
            ))
        };
        for name in query.capture_names() {
            if name.starts_with('_') || matches!(*name, "fold" | "fold.open" | "fold.close") {
                continue;
            }
            let source = sources
                .iter()
                .find(|source| source.text.contains(&format!("@{name}")))
                .expect("a capture name appears in the text that declared it");
            return Err(QueryError(format!(
                "{}: unsupported capture @{name}; use an underscore prefix for helper captures",
                source.name
            )));
        }
        let mut patterns = Vec::new();
        for index in 0..query.pattern_count() {
            let mut pattern = Pattern {
                tags: Vec::new(),
                source: owner(query.start_byte_for_pattern(index)),
            };
            if !query.property_predicates(index).is_empty() {
                return Err(named(index, "#is? and #is-not? are not supported".into()));
            }
            for property in query.property_settings(index) {
                if property.key.as_ref() != "tag" {
                    return Err(named(
                        index,
                        format!("unsupported #set! property {}", property.key),
                    ));
                }
                if property
                    .capture_id
                    .is_some_and(|id| query.capture_names()[id] != "fold")
                {
                    return Err(named(
                        index,
                        "tag metadata must target @fold or its pattern".into(),
                    ));
                }
                let tag = property
                    .value
                    .as_deref()
                    .filter(|tag| !tag.is_empty())
                    .ok_or_else(|| named(index, "#set! tag requires a nonempty string".into()))?;
                pattern.tags.push(tag.to_owned());
            }
            if let Some(predicate) = query.general_predicates(index).first() {
                return Err(named(
                    index,
                    format!("unsupported directive #{}", predicate.operator),
                ));
            }
            patterns.push(pattern);
        }
        Ok(Self {
            query,
            patterns,
            sources: sources.iter().map(|source| source.name.clone()).collect(),
        })
    }
}
