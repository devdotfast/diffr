//! One configuration, layered: bundled defaults, the user's global file,
//! the repository's `diffr.toml`, `DIFFR_*` environment variables, then
//! `--set` overrides. Every field carries a doc comment, which becomes its
//! description in `diffr config schema`.
pub(crate) mod query;
pub(crate) mod store;
use crate::hash::DftHashMap;
use crate::parse::{guess_language::Language, tree_sitter_parser};
use figment::providers::{Env, Format, Serialized, Toml};
use figment::Figment;
use query::AnnotationQuery;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use strum::IntoEnumIterator;

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Config {
    /// Tree-sitter fold and context queries per language, keyed by the
    /// lowercase language name. Omitted queries keep the bundled ones; an
    /// empty string disables that feature.
    #[schemars(skip)]
    pub(crate) languages: BTreeMap<String, LanguageConfig>,
    /// What gets folded and what starts collapsed.
    pub(crate) folds: FoldsConfig,
    /// Pseudocode summaries for large new function bodies.
    pub(crate) summarize: SummarizeConfig,
    /// Colors for the terminal frontend.
    pub(crate) theme: ThemeConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct FoldsConfig {
    /// Bodies shorter than this are never summarized or collapsed by a rule.
    pub(crate) min_lines: usize,
    /// Collapse deleted function bodies, keeping their header line visible.
    pub(crate) collapse_deleted: bool,
    /// Hide files classified as generated, such as lockfiles and build output.
    pub(crate) collapse_generated: bool,
    /// Hide files classified as tests.
    pub(crate) collapse_tests: bool,
    /// Unchanged lines kept visible on either side of a change. `-U` overrides it.
    pub(crate) context_lines: u32,
    /// An external JSON-RPC summarizer, run after the built-in one.
    #[schemars(skip)]
    pub(crate) hook: Option<HookConfig>,
}

impl Default for FoldsConfig {
    fn default() -> Self {
        Self {
            min_lines: 12,
            collapse_deleted: true,
            collapse_generated: true,
            collapse_tests: true,
            context_lines: 3,
            hook: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct SummarizeConfig {
    /// Summarize large new function bodies as pseudocode. Silently off
    /// without an API key.
    pub(crate) enabled: bool,
    /// Which model API to call.
    pub(crate) provider: Provider,
    /// The model name sent to the provider.
    pub(crate) model: String,
    /// The provider's API key. `GEMINI_API_KEY` or `GOOGLE_API_KEY` in the
    /// environment is used when this is unset.
    pub(crate) api_key: Option<String>,
    /// Override the provider's base URL, for proxies and tests.
    pub(crate) endpoint: Option<String>,
    /// Per-request limit in milliseconds.
    pub(crate) timeout_ms: u64,
    /// Requests in flight at once across files.
    pub(crate) max_concurrency: usize,
    /// Retries after a timeout, rate limit, or server error before the run
    /// is aborted.
    pub(crate) retries: u32,
}

impl Default for SummarizeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            provider: Provider::Gemini,
            model: "gemini-3.8-flash".to_owned(),
            api_key: None,
            endpoint: None,
            timeout_ms: 60_000,
            max_concurrency: 16,
            retries: 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
#[schemars(inline)]
pub(crate) enum Provider {
    Gemini,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ThemeConfig {
    /// A bundled theme name.
    pub(crate) name: String,
    /// A Helix-style theme file that replaces the bundled theme.
    pub(crate) path: Option<PathBuf>,
}

impl Default for ThemeConfig {
    fn default() -> Self {
        Self {
            name: "default-dark".to_owned(),
            path: None,
        }
    }
}

/// A trusted subprocess that supplies summaries for large novel folds.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct HookConfig {
    /// Relative command paths resolve against the file that configured the
    /// hook, or the workspace when it came from the environment.
    #[serde(skip)]
    pub(crate) dir: PathBuf,
    pub(crate) command: Vec<String>,
    /// None sends every tagged fold; otherwise a fold needs one of these tags.
    #[serde(default)]
    pub(crate) tags: Option<Vec<String>>,
    /// Overrides `folds.min_lines` for the hook alone.
    #[serde(default)]
    pub(crate) min_lines: Option<usize>,
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

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
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
    pub(crate) folds: FoldsConfig,
    pub(crate) summarize: SummarizeConfig,
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

/// Where the layers come from.
pub(crate) struct Sources<'a> {
    /// The repository root; `diffr.toml` inside it is the repository layer.
    pub(crate) workspace: &'a Path,
    /// Replaces the global file. Must exist.
    pub(crate) explicit: Option<&'a Path>,
    /// `key=value` overrides, applied last.
    pub(crate) overrides: &'a [String],
}

/// The user's global file: `$XDG_CONFIG_HOME/diffr/config.toml`, falling
/// back to `~/.config/diffr/config.toml`.
pub(crate) fn global_path() -> Result<PathBuf, ConfigError> {
    let dir = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => dirs::home_dir()
            .ok_or_else(|| ConfigError("no home directory for this user".into()))?
            .join(".config"),
    };
    Ok(dir.join("diffr").join("config.toml"))
}

/// A `key=value` override. Values parse as TOML; anything that is not
/// valid TOML is a string.
pub(crate) fn parse_override(text: &str) -> Result<(String, toml::Value), ConfigError> {
    let (key, value) = text
        .split_once('=')
        .ok_or_else(|| ConfigError(format!("--set expects key=value, got {text:?}")))?;
    Ok((key.to_owned(), parse_value(value)))
}

/// TOML scalars and arrays; anything else is the literal string.
pub(crate) fn parse_value(text: &str) -> toml::Value {
    match toml::from_str::<toml::Table>(&format!("v = {text}")) {
        Ok(mut table) => match table.remove("v") {
            Some(toml::Value::Table(_)) | Some(toml::Value::Datetime(_)) | None => {
                toml::Value::String(text.to_owned())
            }
            Some(value) => value,
        },
        Err(_) => toml::Value::String(text.to_owned()),
    }
}

impl Config {
    /// Layer every source and resolve it. Missing global and repository
    /// files are fine; an explicit `--config` file must exist.
    pub(crate) fn load(sources: Sources<'_>) -> Result<Self, ConfigError> {
        let global = match sources.explicit {
            Some(path) => {
                if !path.is_file() {
                    return Err(ConfigError(format!("{}: not found", path.display())));
                }
                path.to_path_buf()
            }
            None => global_path()?,
        };
        let overrides = sources
            .overrides
            .iter()
            .map(|text| parse_override(text))
            .collect::<Result<Vec<_>, _>>()?;
        let figment = |overrides: &[(String, toml::Value)]| {
            let mut figment = Figment::from(Serialized::defaults(Config::default()))
                .merge(Toml::file(&global))
                .merge(Toml::file(sources.workspace.join("diffr.toml")))
                .merge(
                    Env::prefixed("DIFFR_")
                        .filter(|key| key.as_str().contains("__"))
                        .split("__"),
                );
            for (key, value) in overrides {
                figment = figment.merge(Serialized::default(key, value));
            }
            figment
        };
        let typed = figment(&overrides);
        let (figment, mut config) = match typed.extract::<Config>() {
            Ok(config) => (typed, config),
            Err(error) => {
                // A value like `1234` for a string key parsed as a number; the
                // literal text is the intended value.
                let as_strings: Vec<_> = sources
                    .overrides
                    .iter()
                    .map(|text| {
                        let (key, value) = text.split_once('=').expect("validated above");
                        (key.to_owned(), toml::Value::String(value.to_owned()))
                    })
                    .collect();
                let retry = figment(&as_strings);
                match retry.extract::<Config>() {
                    Ok(config) if as_strings != overrides => (retry, config),
                    _ => return Err(ConfigError(error.to_string())),
                }
            }
        };
        if let Some(hook) = &mut config.folds.hook {
            hook.dir = figment
                .find_metadata("folds.hook.command")
                .and_then(|metadata| metadata.source.as_ref())
                .and_then(|source| source.file_path())
                .and_then(|path| path.parent())
                .map(Path::to_path_buf)
                .unwrap_or_else(|| sources.workspace.to_path_buf());
        }
        Ok(config)
    }

    pub(crate) fn from_toml(source: &str) -> Result<Self, ConfigError> {
        toml::from_str(source).map_err(|error| ConfigError(error.to_string()))
    }

    /// The JSON Schema of the configuration, with a description and default
    /// on every setting.
    pub(crate) fn schema() -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(Config)).expect("schema serializes")
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
        if self.summarize.timeout_ms == 0 || self.summarize.max_concurrency == 0 {
            return Err(ConfigError(
                "summarize.timeout_ms and summarize.max_concurrency must be positive".into(),
            ));
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
            folds: self.folds.clone(),
            summarize: self.summarize,
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
            (Some(30), 5000, 30_000)
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
    fn javascript_test_callbacks_are_tagged() {
        let params = Params::default();
        for (path, source) in [
            (
                "a.test.ts",
                "it('adds', () => {\n  expect(1).toBe(1);\n});\n",
            ),
            ("a.test.js", "describe('x', function () {\n  run();\n});\n"),
            (
                "a.test.tsx",
                "test('y', async () => {\n  await run();\n});\n",
            ),
        ] {
            let result = DiffResult::from_sources_with_params(path, "", source, &params);
            assert!(
                result
                    .rhs_folds
                    .iter()
                    .any(|fold| fold.tags == ["body", "test"]),
                "{path}: {:?}",
                result.rhs_folds.iter().map(|f| &f.tags).collect::<Vec<_>>()
            );
        }
        let plain = DiffResult::from_sources_with_params(
            "a.ts",
            "",
            "run('z', () => {\n  go();\n});\n",
            &params,
        );
        assert!(plain
            .rhs_folds
            .iter()
            .all(|fold| !fold.tags.iter().any(|tag| tag == "test")));
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

#[cfg(test)]
mod layer_tests {
    use super::*;

    fn load(dir: &Path, overrides: &[&str]) -> Result<Config, ConfigError> {
        let overrides: Vec<String> = overrides.iter().map(|s| (*s).to_owned()).collect();
        Config::load(Sources {
            workspace: dir,
            explicit: Some(&dir.join("global.toml")),
            overrides: &overrides,
        })
    }

    #[test]
    fn layers_resolve_in_order() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("global.toml"),
            "[folds]\nmin_lines = 5\ncollapse_tests = false\n[summarize]\nmodel = 'global'\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("diffr.toml"),
            "[summarize]\nmodel = 'repo'\n",
        )
        .unwrap();
        let config = load(dir.path(), &["folds.min_lines=7"]).unwrap();
        assert_eq!(config.folds.min_lines, 7);
        assert!(!config.folds.collapse_tests);
        assert!(config.folds.collapse_deleted);
        assert_eq!(config.summarize.model, "repo");
        assert_eq!(config.summarize.retries, 3);
    }

    #[test]
    fn overrides_that_look_numeric_still_fill_string_keys() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("global.toml"), "").unwrap();
        let config = load(dir.path(), &["summarize.api_key=1234"]).unwrap();
        assert_eq!(config.summarize.api_key.as_deref(), Some("1234"));
        let config = load(dir.path(), &["summarize.api_key=abc-def"]).unwrap();
        assert_eq!(config.summarize.api_key.as_deref(), Some("abc-def"));
        let config = load(dir.path(), &["folds.collapse_deleted=false"]).unwrap();
        assert!(!config.folds.collapse_deleted);
    }

    #[test]
    fn unknown_keys_and_missing_explicit_files_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("global.toml"), "").unwrap();
        assert!(load(dir.path(), &["folds.typo=1"]).is_err());
        assert!(load(dir.path(), &["folds.min_lines=abc"]).is_err());
        assert!(load(dir.path(), &["folds.min_lines"]).is_err());
        assert!(Config::load(Sources {
            workspace: dir.path(),
            explicit: Some(&dir.path().join("absent.toml")),
            overrides: &[],
        })
        .is_err());
    }

    #[test]
    fn hook_paths_resolve_against_the_file_that_configured_it() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("elsewhere");
        std::fs::create_dir(&nested).unwrap();
        std::fs::write(
            nested.join("global.toml"),
            "[folds.hook]\ncommand = ['hook.py']\n",
        )
        .unwrap();
        let config = Config::load(Sources {
            workspace: dir.path(),
            explicit: Some(&nested.join("global.toml")),
            overrides: &[],
        })
        .unwrap();
        assert_eq!(config.folds.hook.unwrap().dir, nested);
    }

    #[test]
    fn schema_describes_every_setting_with_its_default() {
        let schema = Config::schema();
        let folds = &schema["properties"]["folds"];
        let folds = match folds.get("$ref") {
            Some(reference) => {
                let name = reference.as_str().unwrap().rsplit('/').next().unwrap();
                &schema["$defs"][name]
            }
            None => folds,
        };
        let min_lines = &folds["properties"]["min_lines"];
        assert_eq!(min_lines["default"], 12);
        assert!(min_lines["description"]
            .as_str()
            .unwrap()
            .contains("never summarized"));
        assert!(schema["properties"].get("languages").is_none());
        assert!(folds["properties"].get("hook").is_none());
    }
}
