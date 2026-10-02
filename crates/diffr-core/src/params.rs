//! What the engine needs to diff one file, and nothing about where it came
//! from: the compiled fold query of each language and the limits of one
//! comparison. Reading the config file, building the plugin pipeline and
//! parsing flags all happen before these are made (see [`crate::config`]).
pub mod query;

use crate::hash::DftHashMap;
use crate::parse::{guess_language::Language, tree_sitter_parser};
use query::AnnotationQuery;
use std::sync::{Arc, OnceLock};
use strum::IntoEnumIterator;

pub const DEFAULT_BYTE_LIMIT: usize = 1_000_000;
// Chosen experimentally: this is sufficiently many for all the sample
// files (the highest is slow_1.rs/slow_2.rs at 1.3M nodes), but
// small enough to terminate in ~5 seconds like the test file in #306.
pub const DEFAULT_GRAPH_LIMIT: usize = 3_000_000;
pub const DEFAULT_PARSE_ERROR_LIMIT: usize = 0;

#[derive(Debug, Clone)]
pub struct DiffOptions {
    pub graph_limit: usize,
    pub byte_limit: usize,
    pub parse_error_limit: usize,
    pub ignore_comments: bool,
    /// The file is tagged `generated`: diff it by line without parsing.
    pub generated: bool,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            graph_limit: DEFAULT_GRAPH_LIMIT,
            byte_limit: DEFAULT_BYTE_LIMIT,
            parse_error_limit: DEFAULT_PARSE_ERROR_LIMIT,
            ignore_comments: false,
            generated: false,
        }
    }
}

/// Every language's parser and fold query. A language no plugin wrote a
/// query for gets an empty one, and its grammar loads the first time a file
/// needs it.
pub struct Params {
    languages: DftHashMap<Language, OnceLock<Arc<LanguageParams>>>,
}

pub struct LanguageParams {
    pub parser: &'static tree_sitter_parser::TreeSitterConfig,
    /// The fold and context queries of every enabled plugin, concatenated.
    pub query: AnnotationQuery,
    sub_languages: OnceLock<
        Vec<(
            &'static tree_sitter_parser::TreeSitterSubLanguage,
            Arc<LanguageParams>,
        )>,
    >,
}

impl LanguageParams {
    pub fn sub_languages(
        &self,
    ) -> &[(
        &'static tree_sitter_parser::TreeSitterSubLanguage,
        Arc<LanguageParams>,
    )] {
        self.sub_languages.get().expect("resolved sub-languages")
    }
}

impl Params {
    /// Params with these compiled fold queries, and empty ones for every
    /// other language.
    pub fn new(queries: impl IntoIterator<Item = (Language, AnnotationQuery)>) -> Self {
        let mut languages: DftHashMap<_, _> = Language::iter()
            .map(|language| (language, OnceLock::new()))
            .collect();
        for (language, query) in queries {
            languages.insert(
                language,
                OnceLock::from(Arc::new(LanguageParams {
                    parser: tree_sitter_parser::from_language(language),
                    query,
                    sub_languages: OnceLock::new(),
                })),
            );
        }
        Self { languages }
    }

    pub fn language(&self, language: Language) -> &Arc<LanguageParams> {
        let config = self.languages[&language].get_or_init(|| {
            // Languages without annotation rules still support structural diffing.
            // Keep their grammars lazy, as in the existing parser registry.
            let parser = tree_sitter_parser::from_language(language);
            Arc::new(LanguageParams {
                parser,
                query: AnnotationQuery::compile(&parser.language, &[]).expect("an empty query"),
                sub_languages: OnceLock::new(),
            })
        });
        config.sub_languages.get_or_init(|| {
            config
                .parser
                .sub_languages
                .iter()
                .map(|sub| (sub, Arc::clone(self.language(sub.parse_as))))
                .collect()
        });
        config
    }
}
