//! Deserialize user settings, resolve defaults, and compile once before diffing.
use crate::hash::DftHashMap;
use crate::parse::{guess_language::Language, tree_sitter_parser};
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
    queries: DftHashMap<tree_sitter::Language, tree_sitter::Query>,
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
                validate(&grammar, &source)
                    .map_err(|error| ConfigError(format!("languages.{name}.{feature}: {error}")))?;
                sources.push(source);
            }
            let query = tree_sitter::Query::new(&grammar, &sources.join("\n"))
                .map_err(|error| ConfigError(format!("languages.{name}: {error}")))?;
            queries.insert(grammar, query);
        }
        Ok(Params { queries })
    }
}

fn validate(grammar: &tree_sitter::Language, source: &str) -> Result<(), ConfigError> {
    let query =
        tree_sitter::Query::new(grammar, source).map_err(|error| ConfigError(error.to_string()))?;
    for name in query.capture_names() {
        if matches!(
            *name,
            "fold.body"
                | "fold.collection"
                | "fold.import"
                | "fold.test"
                | "fold.comment"
                | "fold.string"
                | "context.scope"
                | "context.body"
                | "context.close"
                | "context.indented_body"
                | "context.boundary"
                | "name"
                | "attribute"
        ) {
            continue;
        }
        return Err(ConfigError(format!("unsupported capture @{name}")));
    }
    for pattern in 0..query.pattern_count() {
        if !query.general_predicates(pattern).is_empty()
            || !query.property_settings(pattern).is_empty()
            || !query.property_predicates(pattern).is_empty()
        {
            return Err(ConfigError(format!(
                "unsupported directive in pattern {}",
                pattern + 1
            )));
        }
    }
    Ok(())
}

impl Params {
    pub(crate) fn query(&self, language: &tree_sitter::Language) -> Option<&tree_sitter::Query> {
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
