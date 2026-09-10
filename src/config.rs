//! Deserialize user settings, resolve defaults, and compile once before diffing.
pub(crate) mod query;
use crate::hash::DftHashMap;
use crate::parse::{guess_language::Language, tree_sitter_parser};
use query::AnnotationQuery;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use strum::IntoEnumIterator;

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Config {
    pub(crate) languages: BTreeMap<String, LanguageConfig>,
    pub(crate) folds: FoldsConfig,
}

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct FoldsConfig {
    pub(crate) hook: Option<HookConfig>,
}

/// A trusted subprocess that supplies summaries for large novel folds.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct HookConfig {
    /// Relative command paths resolve against the config file, wherever it lives.
    #[serde(skip)]
    pub(crate) dir: PathBuf,
    pub(crate) command: Vec<String>,
    /// None sends every tagged fold; otherwise a fold needs one of these tags.
    #[serde(default)]
    pub(crate) tags: Option<Vec<String>>,
    #[serde(default)]
    pub(crate) min_lines: usize,
    /// Per-call limit once the hook is listening.
    #[serde(default = "default_timeout_ms")]
    pub(crate) timeout_ms: u64,
    /// How long the hook may take to start listening on its port.
    #[serde(default = "default_startup_timeout_ms")]
    pub(crate) startup_timeout_ms: u64,
}

fn default_timeout_ms() -> u64 {
    5000
}

fn default_startup_timeout_ms() -> u64 {
    30_000
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
    languages: DftHashMap<Language, OnceLock<Arc<LanguageParams>>>,
    pub(crate) hook: Option<HookConfig>,
}

pub(crate) struct LanguageParams {
    pub(crate) parser: &'static tree_sitter_parser::TreeSitterConfig,
    pub(crate) folds: AnnotationQuery,
    pub(crate) context: AnnotationQuery,
    sub_languages: OnceLock<
        Vec<(
            &'static tree_sitter_parser::TreeSitterSubLanguage,
            Arc<LanguageParams>,
        )>,
    >,
}

impl LanguageParams {
    pub(crate) fn sub_languages(
        &self,
    ) -> &[(
        &'static tree_sitter_parser::TreeSitterSubLanguage,
        Arc<LanguageParams>,
    )] {
        self.sub_languages.get().expect("resolved sub-languages")
    }
}

impl Config {
    pub(crate) fn load(workspace: &Path, explicit: Option<&Path>) -> Result<Self, ConfigError> {
        let path = explicit
            .map(Path::to_path_buf)
            .unwrap_or_else(|| workspace.join("diffr.toml"));
        match std::fs::read_to_string(&path) {
            Ok(source) => {
                let mut config = Self::from_toml(&source)?;
                if let Some(hook) = &mut config.folds.hook {
                    hook.dir = path
                        .parent()
                        .expect("config file has a parent")
                        .to_path_buf();
                }
                Ok(config)
            }
            Err(error) if explicit.is_none() && error.kind() == std::io::ErrorKind::NotFound => {
                Ok(Self::default())
            }
            Err(error) => Err(ConfigError(format!("{}: {error}", path.display()))),
        }
    }

    pub(crate) fn from_toml(source: &str) -> Result<Self, ConfigError> {
        toml::from_str(source).map_err(|error| ConfigError(error.to_string()))
    }

    pub(crate) fn compile(self) -> Result<Params, ConfigError> {
        if let Some(hook) = &self.folds.hook {
            if hook.command.is_empty() {
                return Err(ConfigError("folds.hook.command must not be empty".into()));
            }
            if hook.timeout_ms == 0 || hook.startup_timeout_ms == 0 {
                return Err(ConfigError("folds.hook timeouts must be positive".into()));
            }
        }
        let defaults = Self::from_toml(include_str!("config/defaults.toml"))?;
        let mut resolved = defaults.languages;
        for (name, overrides) in self.languages {
            let target = resolved.entry(name).or_default();
            if overrides.folds.is_some() {
                target.folds = overrides.folds;
            }
            if overrides.context.is_some() {
                target.context = overrides.context;
            }
        }
        let mut languages: DftHashMap<_, _> = Language::iter()
            .map(|language| (language, OnceLock::new()))
            .collect();
        for (name, config) in resolved {
            let language = Language::iter()
                .find(|language| format!("{language:?}").to_lowercase() == name)
                .ok_or_else(|| ConfigError(format!("unknown language: {name}")))?;
            let parser = tree_sitter_parser::from_language(language);
            let compile = |feature, source: Option<String>| {
                AnnotationQuery::compile(&parser.language, source.as_deref().unwrap_or_default())
                    .map_err(|error| ConfigError(format!("languages.{name}.{feature}: {error}")))
            };
            languages.insert(
                language,
                OnceLock::from(Arc::new(LanguageParams {
                    parser,
                    folds: compile("folds", config.folds)?,
                    context: compile("context", config.context)?,
                    sub_languages: OnceLock::new(),
                })),
            );
        }
        Ok(Params {
            languages,
            hook: self.folds.hook,
        })
    }
}

impl Params {
    pub(crate) fn language(&self, language: Language) -> &Arc<LanguageParams> {
        let config = self.languages[&language].get_or_init(|| {
            // Languages without annotation rules still support structural diffing.
            // Keep their grammars lazy, as in the existing parser registry.
            let parser = tree_sitter_parser::from_language(language);
            Arc::new(LanguageParams {
                parser,
                folds: AnnotationQuery::compile(&parser.language, "").expect("empty fold query"),
                context: AnnotationQuery::compile(&parser.language, "")
                    .expect("empty context query"),
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
    fn language_without_annotation_rules_keeps_structural_diffing() {
        let params = Params::default();
        let result = DiffResult::from_sources_with_params(
            "a.c",
            "int run() { return 1; }",
            "int run() { return 2; }",
            &params,
        );
        assert!(matches!(
            result.file_format,
            crate::summary::FileFormat::SupportedLanguage(Language::C)
        ));
        assert!(result.has_syntactic_changes);
        assert!(!result.rhs_positions.is_empty());
        assert!(result.rhs_folds.is_empty());
    }

    #[test]
    fn shared_grammars_keep_language_configuration_independent() {
        let params = Config::from_toml(
            r#"
            [languages.javascript]
            folds = '''((statement_block) @fold (#set! tag "plain-js"))'''
            [languages.javascriptjsx]
            folds = '''((statement_block) @fold (#set! tag "jsx"))'''
        "#,
        )
        .unwrap()
        .compile()
        .unwrap();
        for (path, tag) in [("file.js", "plain-js"), ("file.jsx", "jsx")] {
            let result = DiffResult::from_sources_with_params(
                path,
                "",
                "function run() { work(); }",
                &params,
            );
            assert_eq!(result.rhs_folds.len(), 1);
            assert_eq!(result.rhs_folds[0].tags, [tag]);
        }
    }

    #[test]
    fn embedded_languages_use_the_configured_queries() {
        let params = Config::from_toml(
            "[languages.javascript]\nfolds = '((statement_block) @fold (#set! tag embedded))'",
        )
        .unwrap()
        .compile()
        .unwrap();
        let result = DiffResult::from_sources_with_params(
            "page.html",
            "",
            "<script>function run() { work(); }</script>",
            &params,
        );
        assert_eq!(result.rhs_folds.len(), 1);
        assert_eq!(result.rhs_folds[0].tags, ["embedded"]);
    }

    #[test]
    fn parses_fold_hook_settings() {
        let params = Config::from_toml(
            "[folds.hook]\ncommand = ['uv', 'run', 'summarize.py']\ntags = ['body']\nmin_lines = 30",
        )
        .unwrap()
        .compile()
        .unwrap();
        let hook = params.hook.unwrap();
        assert_eq!(hook.command, ["uv", "run", "summarize.py"]);
        assert_eq!(hook.tags.as_deref(), Some(&["body".to_owned()][..]));
        assert_eq!(
            (hook.min_lines, hook.timeout_ms, hook.startup_timeout_ms),
            (30, 5000, 30_000)
        );
        assert!(Config::from_toml("")
            .unwrap()
            .compile()
            .unwrap()
            .hook
            .is_none());
        for input in [
            "[folds.hook]\ncommand = []",
            "[folds.hook]\ncommand = ['x']\ntimeout_ms = 0",
        ] {
            assert!(
                Config::from_toml(input).unwrap().compile().is_err(),
                "{input}"
            );
        }
        assert!(Config::from_toml("[folds.hook]\ncommand = ['x']\nunknown = 1").is_err());
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
    fn arbitrary_tags_and_delimiter_captures_reach_the_domain() {
        let params = configured(
            r#"((block "{" @fold.open "}" @fold.close) @fold (#set! tag "user.validation"))"#,
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
            "((block) @fold (#offset! @fold 0 1 0 -1))",
            "((block) @fold (#unknown! @fold))",
            "((block) @fold (#make-range! \"fold\" @fold @fold))",
            "((block) @fold (#set! typo value))",
            "((block) @fold (#set! tag))",
        ] {
            let config =
                Config::from_toml(&format!("[languages.rust]\nfolds = '''{query}'''")).unwrap();
            let error = match config.compile() {
                Ok(_) => panic!("accepted {query}"),
                Err(error) => error,
            };
            assert!(error.to_string().contains("languages.rust.folds:"));
        }
    }

    #[test]
    fn rust_labeled_blocks_fold_between_actual_braces() {
        let params = Params::default();
        for source in [
            "fn f() { 'outer: { work(); } }",
            "fn f() { 'outer: /* prefix */ { work(); } }",
            "fn f() { { work(); } }",
        ] {
            let result = DiffResult::from_sources_with_params("a.rs", "", source, &params);
            let start = source.find("{ work(); }").unwrap() + 1;
            let end = start + " work(); ".len();
            assert!(
                result.rhs_folds.iter().any(|fold| {
                    fold.range.start.line.as_usize() == 0
                        && fold.range.start.byte_column == start
                        && fold.range.end.byte_column == end
                }),
                "missing inner body in {source}"
            );
        }
    }

    #[test]
    fn unicode_string_fold_uses_the_complete_node_range() {
        let params = configured("(string_literal) @fold", "");
        let source = "fn f() { let x = \"☕\"; }";
        let diff = DiffResult::from_sources_with_params("a.rs", "", source, &params);
        assert_eq!(diff.rhs_folds.len(), 1);
        let range = diff.rhs_folds[0].range;
        assert_eq!(
            &source[range.start.byte_column..range.end.byte_column],
            "\"☕\""
        );
    }

    #[test]
    fn neovim_header_end_and_final_have_distinct_endpoints() {
        use crate::parse::{context, guess_language::Language, tree_sitter_parser as parser};
        let src = "fn f(\n    x: i32,\n) {\n    work(x);\n}\n";
        let grammar = parser::from_language(Language::Rust);
        let tree = parser::to_tree(src, grammar);
        for (query, last_header) in [
            ("(function_item body: (block) @context.end) @context", 2),
            ("(function_item body: (block) @context.final) @context", 4),
            ("(function_item) @context", 0),
        ] {
            let params = configured("", query);
            let contexts =
                context::classify(&tree, src, Some(&params.language(Language::Rust).context));
            let context = contexts.values().next().unwrap().first().unwrap();
            assert_eq!(context.header, 0..=last_header);
        }
    }
}

#[cfg(test)]
mod tag_tests {
    use super::*;
    use crate::summary::DiffResult;

    #[test]
    fn repeated_rules_accumulate_sorted_tags_without_duplicate_folds() {
        let query = r#"
            ((block) @fold (#set! tag "user.check"))
            ((block) @fold (#set! tag "body"))
            ((block) @fold (#set! tag "user.check"))
        "#;
        let params = Config::from_toml(&format!("[languages.rust]\nfolds = '''{query}'''"))
            .unwrap()
            .compile()
            .unwrap();
        let result =
            DiffResult::from_sources_with_params("a.rs", "", "fn f() { work(); }", &params);
        assert_eq!(result.rhs_folds.len(), 1);
        assert_eq!(result.rhs_folds[0].tags, ["body", "user.check"]);
    }

    #[test]
    fn conflicting_ranges_do_not_depend_on_query_order() {
        let whole = "((block) @fold (#set! tag \"whole\"))";
        let interior = "((block \"{\" @fold.open \"}\" @fold.close) @fold (#set! tag \"inside\"))";
        for query in [
            format!("{whole}\n{interior}"),
            format!("{interior}\n{whole}"),
        ] {
            let params = Config::from_toml(&format!("[languages.rust]\nfolds = '''{query}'''"))
                .unwrap()
                .compile()
                .unwrap();
            let result =
                DiffResult::from_sources_with_params("a.rs", "", "fn f() { work(); }", &params);
            assert!(result.rhs_folds.is_empty());
        }
    }

    #[test]
    fn test_bodies_keep_both_tags_and_remain_paired() {
        use crate::parse::folds::FoldMatch;
        let params = Params::default();
        let result = DiffResult::from_sources_with_params(
            "a.rs",
            "#[test]\nfn example() { old(); }",
            "#[test]\nfn example() { old(); new(); }",
            &params,
        );
        assert_eq!(result.lhs_folds.len(), 1);
        assert_eq!(result.rhs_folds.len(), 1);
        assert_eq!(result.lhs_folds[0].tags, ["body", "test"]);
        assert_eq!(result.rhs_folds[0].tags, ["body", "test"]);
        assert!(matches!(&result.lhs_folds[0].match_kind,
            FoldMatch::Unchanged { opposite } if *opposite == result.rhs_folds[0].range));
    }
}
