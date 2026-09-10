//! Deserialize user settings, resolve defaults, and compile once before diffing.
pub(crate) mod query;
use crate::hash::DftHashMap;
use crate::parse::{guess_language::Language, tree_sitter_parser};
use query::AnnotationQuery;
use serde::Deserialize;
use std::collections::BTreeMap;
use strum::IntoEnumIterator;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Config {
    pub(crate) languages: BTreeMap<String, LanguageConfig>,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct LanguageConfig {
    /// None keeps the bundled query; an empty string disables this feature.
    pub(crate) folds: Option<String>,
    pub(crate) context: Option<String>,
}

#[derive(Debug)]
pub(crate) struct ConfigError(String);
impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for ConfigError {}

pub(crate) struct Params {
    queries: DftHashMap<tree_sitter::Language, AnnotationQuery>,
}

impl Config {
    pub(crate) fn from_toml(source: &str) -> Result<Self, ConfigError> {
        toml::from_str(source).map_err(|error| ConfigError(error.to_string()))
    }

    pub(crate) fn compile(self) -> Result<Params, ConfigError> {
        let mut resolved = Self::from_toml(include_str!("config/defaults.toml"))?.languages;
        for (name, overrides) in self.languages {
            let target = resolved.entry(name).or_default();
            if overrides.folds.is_some() {
                target.folds = overrides.folds;
            }
            if overrides.context.is_some() {
                target.context = overrides.context;
            }
        }
        let mut queries = DftHashMap::default();
        for (name, config) in resolved {
            let language = Language::iter()
                .find(|language| format!("{language:?}").to_lowercase() == name)
                .ok_or_else(|| ConfigError(format!("unknown language: {name}")))?;
            let grammar = tree_sitter_parser::from_language(language).language.clone();
            let mut sources = Vec::new();
            for (feature, source) in [("folds", config.folds), ("context", config.context)] {
                let Some(source) = source else {
                    continue;
                };
                AnnotationQuery::compile(&grammar, &source)
                    .map_err(|error| ConfigError(format!("languages.{name}.{feature}: {error}")))?;
                sources.push(source);
            }
            let query = AnnotationQuery::compile(&grammar, &sources.join("\n"))
                .map_err(|error| ConfigError(format!("languages.{name}: {error}")))?;
            queries.insert(grammar, query);
        }
        Ok(Params { queries })
    }
}

impl Params {
    pub(crate) fn query(&self, language: &tree_sitter::Language) -> Option<&AnnotationQuery> {
        self.queries.get(language)
    }
}

impl Default for Params {
    fn default() -> Self {
        Config::default()
            .compile()
            .expect("invalid bundled annotation configuration")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::summary::DiffResult;

    #[test]
    fn configuration_is_independent_and_omission_keeps_other_defaults() {
        let custom = Config::from_toml("[languages.rust]\nfolds = ''")
            .unwrap()
            .compile()
            .unwrap();
        let defaults = Params::default();
        let source = "fn f() { work(); }";
        let custom_result = DiffResult::from_sources_with_params("a.rs", "", source, &custom);
        let default_result = DiffResult::from_sources_with_params("a.rs", "", source, &defaults);
        assert!(custom_result.rhs_folds.is_empty());
        assert!(!default_result.rhs_folds.is_empty());
        let python = DiffResult::from_sources_with_params("a.py", "", "import os\n", &custom);
        assert!(!python.rhs_folds.is_empty());
    }

    #[test]
    fn rejects_unknown_settings_languages_and_invalid_queries() {
        assert!(Config::from_toml("typo = true").is_err());
        for input in [
            "[languages.unknown]",
            "[languages.rust]\nfolds = '(not_a_rust_node) @fold.body'",
            "[languages.rust]\nfolds = '(block) @typo'",
        ] {
            assert!(
                Config::from_toml(input).unwrap().compile().is_err(),
                "{input}"
            );
        }
    }
}

#[cfg(test)]
mod query_tests {
    use super::*;
    use crate::summary::DiffResult;

    fn configured(folds: &str, context: &str) -> Params {
        Config::from_toml(&format!(
            "[languages.rust]\nfolds = '''{folds}'''\ncontext = '''{context}'''"
        ))
        .unwrap()
        .compile()
        .unwrap()
    }

    #[test]
    fn arbitrary_tags_and_byte_offsets_reach_the_domain() {
        let params = configured(
            r#"((block) @fold (#offset! @fold 0 1 0 -1) (#set! tag "user.validation"))"#,
            "",
        );
        let source = "fn f() { println!(\"☕\"); }";
        let diff = DiffResult::from_sources_with_params("a.rs", "", source, &params);
        let fold = &diff.rhs_folds[0];
        assert_eq!(fold.tags, ["user.validation"]);
        assert_eq!(
            &source[fold.range.start.byte_column..fold.range.end.byte_column],
            " println!(\"☕\"); "
        );
    }

    #[test]
    fn rejects_unsupported_or_malformed_directives_at_compile_time() {
        for query in [
            "((block) @fold (#offset! @fold 0 1 0))",
            "((block) @fold (#offset! @fold 0 x 0 0))",
            "((block) @fold (#unknown! @fold))",
            "((block) @fold (#set! typo value))",
            "((block) @fold (#set! tag))",
        ] {
            let config =
                Config::from_toml(&format!("[languages.rust]\nfolds = '''{query}'''")).unwrap();
            let error = match config.compile() {
                Ok(_) => panic!("accepted {query}"),
                Err(error) => error,
            };
            assert!(error.to_string().contains("languages.rust.folds"));
        }
    }

    #[test]
    fn invalid_utf8_offsets_do_not_produce_invalid_source_ranges() {
        let params = configured(r#"((string_literal) @fold (#offset! @fold 0 2 0 -1))"#, "");
        let diff =
            DiffResult::from_sources_with_params("a.rs", "", "fn f() { let x = \"☕\"; }", &params);
        assert!(diff.rhs_folds.is_empty());
    }

    #[test]
    fn neovim_header_end_and_final_have_distinct_endpoints() {
        use crate::parse::{annotations, guess_language::Language, tree_sitter_parser as parser};
        let src = "fn f(\n    x: i32,\n) {\n    work(x);\n}\n";
        let grammar = parser::from_language(Language::Rust);
        let tree = parser::to_tree(src, grammar);
        for (query, last_header) in [
            ("(function_item body: (block) @context.end) @context", 2),
            ("(function_item body: (block) @context.final) @context", 4),
            ("(function_item) @context", 0),
        ] {
            let params = configured("", query);
            let annotations = annotations::collect(&tree, src, params.query(&grammar.language));
            let context = annotations
                .contexts
                .values()
                .next()
                .unwrap()
                .first()
                .unwrap();
            assert_eq!(context.header, 0..=last_header);
        }
    }
}
