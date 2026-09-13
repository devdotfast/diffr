//! One configuration, layered: bundled defaults, the user's global file,
//! the repository's `diffr.toml`, `DIFFR_*` environment variables, then
//! `--set` overrides. Every field carries a doc comment, which becomes its
//! description in `diffr config schema`, and every setting a `title` and an
//! `x-group` that settings screens show in place of the dotted key.
pub(crate) mod query;
pub(crate) mod store;
use crate::hash::DftHashMap;
use crate::options::DiffOptions;
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
    /// Colors for the terminal frontend.
    pub(crate) theme: ThemeConfig,
    /// Limits on the structural comparison itself.
    pub(crate) diff: DiffConfig,
}

/// When a file exceeds one of these, diffr falls back to a line diff for
/// it: the alignment is line-based and `stats.fallback` carries the
/// reason; folds still come from the parse where it succeeded. The `DFT_BYTE_LIMIT`, `DFT_GRAPH_LIMIT` and
/// `DFT_PARSE_ERROR_LIMIT` variables override the file, and the matching
/// command-line flags override both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct DiffConfig {
    /// Files larger than this many bytes on either side get a line diff.
    #[schemars(title = "Largest file to diff structurally (bytes)", extend("x-group" = "Diff limits"))]
    pub(crate) byte_limit: usize,
    /// The largest AST matching graph diffr will explore for one file.
    /// A large change to a large file can exceed it; raising it costs time
    /// and memory on those files only.
    #[schemars(title = "Largest matching graph", extend("x-group" = "Diff limits"))]
    pub(crate) graph_limit: usize,
    /// Files with more tree-sitter parse errors than this get a line diff.
    #[schemars(title = "Parse errors allowed", extend("x-group" = "Diff limits"))]
    pub(crate) parse_error_limit: usize,
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            byte_limit: crate::options::DEFAULT_BYTE_LIMIT,
            graph_limit: crate::options::DEFAULT_GRAPH_LIMIT,
            parse_error_limit: crate::options::DEFAULT_PARSE_ERROR_LIMIT,
        }
    }
}

impl DiffConfig {
    /// The engine options for these limits, with the `DFT_*` variables
    /// applied on top. A variable that is set but not a number is an error.
    pub(crate) fn options(&self, ignore_comments: bool) -> Result<DiffOptions, ConfigError> {
        let limit = |name: &str, configured: usize| -> Result<usize, ConfigError> {
            match std::env::var(name) {
                Ok(text) => text.trim().parse().map_err(|_| {
                    ConfigError(format!(
                        "{name} must be a non-negative integer, got {text:?}"
                    ))
                }),
                Err(std::env::VarError::NotPresent) => Ok(configured),
                Err(std::env::VarError::NotUnicode(_)) => {
                    Err(ConfigError(format!("{name} is not valid UTF-8")))
                }
            }
        };
        Ok(DiffOptions {
            byte_limit: limit("DFT_BYTE_LIMIT", self.byte_limit)?,
            graph_limit: limit("DFT_GRAPH_LIMIT", self.graph_limit)?,
            parse_error_limit: limit("DFT_PARSE_ERROR_LIMIT", self.parse_error_limit)?,
            ignore_comments,
            ..DiffOptions::default()
        })
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct FoldsConfig {
    /// An external JSON-RPC summarizer.
    #[schemars(skip)]
    pub(crate) hook: Option<HookConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ThemeConfig {
    /// A bundled theme name.
    #[schemars(title = "Theme", extend("x-group" = "Appearance"))]
    pub(crate) name: String,
    /// A Helix-style theme file that replaces the bundled theme.
    #[schemars(title = "Theme file", extend("x-group" = "Appearance"))]
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
    // Unread until the hook returns as a fold mutation.
    #[allow(dead_code)]
    pub(crate) hook: Option<HookConfig>,
    pub(crate) diff: DiffConfig,
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
            diff: self.diff,
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
    fn diff_limits_default_and_layer_from_the_file() {
        let defaults = Config::default().diff;
        assert_eq!(defaults.graph_limit, crate::options::DEFAULT_GRAPH_LIMIT);
        assert_eq!(defaults.byte_limit, crate::options::DEFAULT_BYTE_LIMIT);
        let custom = Config::from_toml("[diff]\ngraph_limit = 5").unwrap();
        assert_eq!(custom.diff.graph_limit, 5);
        assert_eq!(custom.diff.byte_limit, defaults.byte_limit);
        let options = custom.diff.options(true).unwrap();
        assert_eq!(options.graph_limit, 5);
        assert!(options.ignore_comments);
        let compiled = custom.compile().unwrap();
        assert_eq!(compiled.diff.graph_limit, 5);
        let schema = Config::schema();
        assert!(
            schema["$defs"]["DiffConfig"]["properties"]["graph_limit"]["description"]
                .as_str()
                .is_some_and(|text| !text.is_empty())
        );
    }

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
        assert!(result.lhs_folds[0].counterpart(&result.rhs_folds).is_some());
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
            "[diff]\ngraph_limit = 5\nbyte_limit = 6\n[theme]\nname = 'global'\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("diffr.toml"), "[theme]\nname = 'repo'\n").unwrap();
        let config = load(dir.path(), &["diff.graph_limit=7"]).unwrap();
        assert_eq!(config.diff.graph_limit, 7);
        assert_eq!(config.diff.byte_limit, 6);
        assert_eq!(
            config.diff.parse_error_limit,
            crate::options::DEFAULT_PARSE_ERROR_LIMIT
        );
        assert_eq!(config.theme.name, "repo");
    }

    #[test]
    fn overrides_that_look_numeric_still_fill_string_keys() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("global.toml"), "").unwrap();
        let config = load(dir.path(), &["theme.name=1234"]).unwrap();
        assert_eq!(config.theme.name, "1234");
        let config = load(dir.path(), &["diff.graph_limit=9"]).unwrap();
        assert_eq!(config.diff.graph_limit, 9);
    }

    #[test]
    fn unknown_keys_and_missing_explicit_files_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("global.toml"), "").unwrap();
        assert!(load(dir.path(), &["diff.typo=1"]).is_err());
        assert!(load(dir.path(), &["diff.graph_limit=abc"]).is_err());
        assert!(load(dir.path(), &["diff.graph_limit"]).is_err());
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
        let diff = &schema["properties"]["diff"];
        let diff = match diff.get("$ref") {
            Some(reference) => {
                let name = reference.as_str().unwrap().rsplit('/').next().unwrap();
                &schema["$defs"][name]
            }
            None => diff,
        };
        let graph_limit = &diff["properties"]["graph_limit"];
        assert_eq!(graph_limit["default"], crate::options::DEFAULT_GRAPH_LIMIT);
        assert!(graph_limit["description"]
            .as_str()
            .unwrap()
            .contains("matching graph"));
        assert!(schema["properties"].get("languages").is_none());
        assert!(schema["properties"]["folds"]
            .get("properties")
            .is_none_or(|properties| properties.get("hook").is_none()));
    }
}
