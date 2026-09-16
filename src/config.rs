//! diffr's configuration comes from three places: the global file
//! (`$XDG_CONFIG_HOME/diffr/config.toml`, or `--config PATH` in its place),
//! command-line flags, and git attributes. This module owns the file: every
//! key it omits keeps its serde default, and an unknown key or mistyped
//! value is an error naming the key's dotted path. Flags and attributes are
//! applied by their callers. Every field carries a doc comment, which becomes
//! its description in `diffr config schema`, and every setting a `title` and
//! an `x-group` that settings screens show in place of the dotted key.
pub(crate) mod query;
pub(crate) mod store;
use crate::hash::DftHashMap;
use crate::options::DiffOptions;
use crate::parse::{guess_language::Language, tree_sitter_parser};
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
    /// Colors for the terminal frontend.
    pub(crate) theme: ThemeConfig,
    /// Limits on the structural comparison itself.
    pub(crate) diff: DiffConfig,
}

/// When a file exceeds one of these, diffr falls back to a line diff for
/// it: the alignment is line-based and `stats.fallback` carries the
/// reason; folds still come from the parse where it succeeded. The matching
/// command-line flags override these for one run.
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
    /// The engine options for these limits.
    pub(crate) fn options(&self, ignore_comments: bool) -> DiffOptions {
        DiffOptions {
            byte_limit: self.byte_limit,
            graph_limit: self.graph_limit,
            parse_error_limit: self.parse_error_limit,
            ignore_comments,
            ..DiffOptions::default()
        }
    }
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

impl Config {
    /// Read the global file, or `explicit` in its place. A missing global
    /// file is the defaults; an explicit file must exist.
    pub(crate) fn load(explicit: Option<&Path>) -> Result<Self, ConfigError> {
        let path = match explicit {
            Some(path) => path.to_path_buf(),
            None => global_path()?,
        };
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) if explicit.is_none() && error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(ConfigError(format!("{}: {error}", path.display()))),
        };
        Self::from_toml(&source)
            .map_err(|error| ConfigError(format!("{}: {error}", path.display())))
    }

    /// Parse one file's text. Errors lead with the dotted path of the key
    /// they concern, such as `diff.typo`.
    pub(crate) fn from_toml(source: &str) -> Result<Self, ConfigError> {
        serde_path_to_error::deserialize(toml::Deserializer::new(source)).map_err(|error| {
            let path = error.path().to_string();
            let message = error.inner().to_string();
            ConfigError(match path.as_str() {
                "." => message,
                _ => format!("{path}: {message}"),
            })
        })
    }

    /// The JSON Schema of the configuration, with a description and default
    /// on every setting.
    pub(crate) fn schema() -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(Config)).expect("schema serializes")
    }

    pub(crate) fn compile(self) -> Result<Params, ConfigError> {
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
        let options = custom.diff.options(true);
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
    use crate::parse::folds::FoldMatch;
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
    fn an_opening_capture_alone_folds_to_the_end_of_the_fold_node() {
        let query =
            r#"((function_definition ":" @fold.open body: (block) @fold) (#set! tag "body"))"#;
        let params = Config::from_toml(&format!("[languages.python]\nfolds = '''{query}'''"))
            .unwrap()
            .compile()
            .unwrap();
        let rhs = "def f(a):\n    x = a\n    return x\n";
        let result = DiffResult::from_sources_with_params("a.py", "", rhs, &params);
        assert_eq!(result.rhs_folds.len(), 1);
        let range = &result.rhs_folds[0].range;
        // From just after the `:` to the end of the block.
        assert_eq!(
            (range.start.line.as_usize(), range.start.byte_column),
            (0, 9)
        );
        assert_eq!((range.end.line.as_usize(), range.end.byte_column), (2, 12));
    }

    #[test]
    fn a_node_captured_with_two_ranges_is_a_query_conflict_naming_both_patterns() {
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
            let conflict = DiffResult::try_from_sources_with_params(
                "src/lib.rs",
                "",
                "fn f() {\n    work();\n}\n",
                &params,
            )
            .expect_err("a conflict");
            assert_eq!(
                conflict.to_string(),
                "src/lib.rs:1: fold query patterns 0 and 1 capture the same block \
                 with different fold ranges"
            );
            // Other files diff as usual.
            assert!(
                DiffResult::try_from_sources_with_params("a.py", "", "x = 1\n", &params).is_ok()
            );
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
        assert!(matches!(
            result.lhs_folds[0].match_kind,
            FoldMatch::Matched { .. }
        ));
    }
}

#[cfg(test)]
mod load_tests {
    use super::*;

    #[test]
    fn the_file_overrides_defaults_key_by_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[diff]\ngraph_limit = 5\nbyte_limit = 6\n[theme]\nname = 'mine'\n",
        )
        .unwrap();
        let config = Config::load(Some(&path)).unwrap();
        assert_eq!(config.diff.graph_limit, 5);
        assert_eq!(config.diff.byte_limit, 6);
        assert_eq!(
            config.diff.parse_error_limit,
            crate::options::DEFAULT_PARSE_ERROR_LIMIT
        );
        assert_eq!(config.theme.name, "mine");
        assert_eq!(config.theme.path, None);
    }

    #[test]
    fn unknown_keys_name_their_path_and_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[diff]\ngraph_limit = 5\ntypo = 1\n").unwrap();
        let error = Config::load(Some(&path)).unwrap_err().to_string();
        assert!(
            error.starts_with(&format!("{}: diff.typo: ", path.display())),
            "{error}"
        );
        std::fs::write(&path, "[diff]\ngraph_limit = 'many'\n").unwrap();
        let error = Config::load(Some(&path)).unwrap_err().to_string();
        assert!(error.contains("diff.graph_limit: "), "{error}");
    }

    #[test]
    fn a_missing_explicit_file_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        assert!(Config::load(Some(&dir.path().join("absent.toml"))).is_err());
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
    }
}
