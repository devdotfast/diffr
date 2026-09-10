//! Compile the supported Tree-sitter capture and directive conventions.
use super::ConfigError;
use tree_sitter::Query;

pub(crate) struct AnnotationQuery {
    pub(crate) query: Query,
    pub(crate) patterns: Vec<Pattern>,
}

#[derive(Default)]
pub(crate) struct Pattern {
    pub(crate) tags: Vec<String>,
}

impl AnnotationQuery {
    pub(crate) fn compile(
        grammar: &tree_sitter::Language,
        source: &str,
    ) -> Result<Self, ConfigError> {
        let query = Query::new(grammar, source).map_err(|error| ConfigError(error.to_string()))?;
        for name in query.capture_names() {
            if name.starts_with('_')
                || matches!(
                    *name,
                    "fold"
                        | "fold.open"
                        | "fold.close"
                        | "context"
                        | "context.start"
                        | "context.end"
                        | "context.final"
                        | "context.last"
                )
            {
                continue;
            }
            return Err(ConfigError(format!(
                "unsupported capture @{name}; use an underscore prefix for helper captures"
            )));
        }
        let mut patterns = Vec::new();
        for index in 0..query.pattern_count() {
            let mut pattern = Pattern::default();
            if !query.property_predicates(index).is_empty() {
                return Err(ConfigError("#is? and #is-not? are not supported".into()));
            }
            for property in query.property_settings(index) {
                if property.key.as_ref() != "tag" {
                    return Err(ConfigError(format!(
                        "unsupported #set! property {}",
                        property.key
                    )));
                }
                if property
                    .capture_id
                    .is_some_and(|id| query.capture_names()[id] != "fold")
                {
                    return Err(ConfigError(
                        "tag metadata must target @fold or its pattern".into(),
                    ));
                }
                let tag = property
                    .value
                    .as_deref()
                    .filter(|tag| !tag.is_empty())
                    .ok_or_else(|| ConfigError("#set! tag requires a nonempty string".into()))?;
                pattern.tags.push(tag.to_owned());
            }
            if let Some(predicate) = query.general_predicates(index).first() {
                return Err(ConfigError(format!(
                    "unsupported directive #{}",
                    predicate.operator
                )));
            }
            patterns.push(pattern);
        }
        Ok(Self { query, patterns })
    }
}
