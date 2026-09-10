//! Validated syntax-annotation inputs shared by all files in a diff operation.
use crate::hash::DftHashMap;
use crate::parse::{guess_language::Language, tree_sitter_parser};

pub(crate) struct Params {
    queries: DftHashMap<tree_sitter::Language, tree_sitter::Query>,
}

impl Params {
    pub(crate) fn query(&self, language: &tree_sitter::Language) -> Option<&tree_sitter::Query> {
        self.queries.get(language)
    }
}

impl Default for Params {
    fn default() -> Self {
        let languages = [
            Language::Rust,
            Language::Python,
            Language::Go,
            Language::JavaScript,
            Language::JavascriptJsx,
            Language::TypeScript,
            Language::TypeScriptTsx,
        ];
        let queries = languages
            .into_iter()
            .map(|language| {
                let grammar = tree_sitter_parser::from_language(language).language.clone();
                let query = tree_sitter::Query::new(
                    &grammar,
                    crate::parse::query_source(language).unwrap(),
                )
                .expect("invalid bundled syntax query");
                (grammar, query)
            })
            .collect();
        Self { queries }
    }
}
