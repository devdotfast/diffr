//! The plugin host and the bundled plugins, run the way the stream runs
//! them: a file projected with the bundled queries, then each plugin's
//! native code through a [`Pipeline`].
mod context;
mod deleted_bodies;
mod group;
mod hide_files;
mod removed_runs;
mod test_bodies;

use super::testing::*;
use super::*;
use crate::config::Config;
use crate::params::DiffOptions;
use crate::protocol::FileRef;
use diffr_plugin_sdk::tree::{docstring_of, has_tag, is_fold, walk};
use diffr_plugin_sdk::{FileEntry, Move, Plugin};
use serde_json::json;

/// For each `deleted-bodies:function` body on the after side, the first line
/// of the body and the lines of its docstring, as `docstring_of` finds it.
fn documented(path: &str, after: &str) -> Vec<(u32, Option<(u32, u32)>)> {
    let (_, sides) = project(path, "", after);
    let sides = trees(&sides);
    let source = rhs(&sides);
    let mut bodies = Vec::new();
    walk(&source.regions, &mut |region| {
        if is_fold(region) && has_tag(region, "deleted-bodies:function") {
            let docstring = docstring_of(source, region, "deleted-bodies").map(|id| {
                let mut lines = None;
                walk(&source.regions, &mut |docstring| {
                    if docstring.id == id {
                        let range = docstring.range.lines();
                        lines = Some((range.start, range.end));
                    }
                });
                lines.expect("the docstring is on this side")
            });
            bodies.push((region.range.start.line, docstring));
        }
    });
    bodies
}

#[test]
fn rust_doc_and_line_comments_above_a_function_are_its_docstring() {
    let after = "fn keep() -> u32 {\n    let x = 1;\n    x\n}\n\n/// Adds one.\n/// Twice, really.\nfn add(\n    a: u32,\n) -> u32 {\n    let b = a;\n    b + 2\n}\n\n// Plain comment.\n// Two lines.\n#[inline]\nfn sub(a: u32) -> u32 {\n    let b = a;\n    b - 1\n}\n\n// One line.\nfn one(a: u32) -> u32 {\n    let b = a;\n    b - 1\n}\n\n/**\n * Block.\n */\nfn block() {\n    x();\n    y();\n}\n";
    assert_eq!(
        documented("a.rs", after),
        [
            (1, None),
            (10, Some((5, 7))),
            (18, Some((14, 16))),
            (24, None),
            (32, Some((28, 31)))
        ],
        "a one-line docstring is not a region"
    );
}

#[test]
fn a_comment_separated_from_the_function_by_code_does_not_count() {
    let after =
        "// About the constant.\n// Really.\nconst X: u32 = 1;\nfn f() -> u32 {\n    let y = X;\n    y\n}\n";
    assert_eq!(documented("a.rs", after), [(4, None)]);
}

#[test]
fn a_docstring_is_not_found_past_a_one_line_function() {
    let after = "/// First.\n/// Documented.\nfn a() {}\nfn b() {\n    x();\n    y();\n}\n";
    assert_eq!(documented("a.rs", after), [(4, None)]);
    let after =
        "// First.\n// Documented.\nexport const a = 1;\nfunction b() {\n  x();\n  y();\n}\n";
    assert_eq!(documented("a.ts", after), [(4, None)]);
}

#[test]
fn a_python_string_first_in_the_body_is_its_docstring() {
    let after =
        "def f(a):\n    \"\"\"Double a.\n\n    Returns an int.\n    \"\"\"\n    return a * 2\n";
    assert_eq!(documented("a.py", after), [(1, Some((1, 5)))]);
}

#[test]
fn go_and_javascript_comment_runs_document_functions() {
    for (path, source, expected) in [
        (
            "a.go",
            "package a\n\n// Sum adds.\n// Twice.\nfunc Sum(a int) int {\n\tb := a\n\treturn a + b\n}\n",
            (5, Some((2, 4))),
        ),
        (
            "a.ts",
            "// Sum adds.\n// Twice.\nexport function sum(a: number) {\n  const b = a;\n  return a + b;\n}\n",
            (3, Some((0, 2))),
        ),
        (
            "a.js",
            "/**\n * Sum adds.\n */\nconst sum = (a) => {\n  const b = a;\n  return a + b;\n};\n",
            (4, Some((0, 3))),
        ),
    ] {
        assert_eq!(documented(path, source), [expected], "{path}");
    }
}

#[test]
fn the_default_pipeline_makes_every_plugin_that_is_on() {
    let pipeline = Pipeline::from_config(
        &PluginsConfig::default(),
        crate::plugin::native::test_environment(),
    )
    .unwrap();
    let made: Vec<&str> = pipeline
        .plugins
        .iter()
        .map(|plugin| &*plugin.name)
        .collect();
    assert_eq!(
        made,
        [
            "context",
            "hide-files",
            "deleted-bodies",
            "test-bodies",
            "removed-runs",
            "group"
        ],
        "the summarizer is off until turned on"
    );
}

/// Test plugins without options.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct NoOptions {}

/// A plugin asking for a move that cannot be carried out.
struct Bad;

impl Plugin for Bad {
    type Options = NoOptions;

    fn new(_: NoOptions) -> anyhow::Result<Self> {
        Ok(Self)
    }

    fn classify(&self, _: &FileEntry) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn mutate(&self, _: &FileEntry, _: &tree::Pairing<tree::Source>) -> anyhow::Result<Vec<Move>> {
        Ok(vec![Move::SetCollapsed((99_999, true))])
    }
}

/// Plugins adding tags: `a` and `z`; then `b`, checking it sees the tags
/// before it; then one that is not a tag.
struct TagsAZ;
struct TagsB;
struct NotATag;

impl Plugin for TagsAZ {
    type Options = NoOptions;

    fn new(_: NoOptions) -> anyhow::Result<Self> {
        Ok(Self)
    }

    fn classify(&self, _: &FileEntry) -> anyhow::Result<Vec<String>> {
        Ok(vec!["a".to_owned(), "z".to_owned()])
    }

    fn mutate(&self, _: &FileEntry, _: &tree::Pairing<tree::Source>) -> anyhow::Result<Vec<Move>> {
        Ok(Vec::new())
    }
}

impl Plugin for TagsB {
    type Options = NoOptions;

    fn new(_: NoOptions) -> anyhow::Result<Self> {
        Ok(Self)
    }

    fn classify(&self, file: &FileEntry) -> anyhow::Result<Vec<String>> {
        assert_eq!(file.tags, ["a", "z"], "a plugin sees the tags before it");
        Ok(vec!["b".to_owned()])
    }

    fn mutate(&self, _: &FileEntry, _: &tree::Pairing<tree::Source>) -> anyhow::Result<Vec<Move>> {
        Ok(Vec::new())
    }
}

impl Plugin for NotATag {
    type Options = NoOptions;

    fn new(_: NoOptions) -> anyhow::Result<Self> {
        Ok(Self)
    }

    fn classify(&self, _: &FileEntry) -> anyhow::Result<Vec<String>> {
        Ok(vec!["Not A Tag".to_owned()])
    }

    fn mutate(&self, _: &FileEntry, _: &tree::Pairing<tree::Source>) -> anyhow::Result<Vec<Move>> {
        Ok(Vec::new())
    }
}

/// A pipeline of test plugins, each made with no options.
fn pipeline(plugins: Vec<(&str, native::Constructor)>) -> Pipeline {
    let mut pipeline = Pipeline::default();
    for (name, create) in plugins {
        pipeline.push(name, json!({}), &create).unwrap();
    }
    pipeline
}

#[test]
fn classifying_plugins_add_tags_in_order_and_a_bad_tag_is_an_error() {
    let (mut file, _) = project("a.rs", "", "");
    file.tags = vec!["z".to_owned()];
    let tagging = pipeline(vec![
        ("first", native::native::<TagsAZ>),
        ("second", native::native::<TagsB>),
    ]);
    assert_eq!(tagging.classify(&file).unwrap(), ["a", "b", "z"]);
    let bad = pipeline(vec![("bad", native::native::<NotATag>)]);
    assert_eq!(
        format!("{:#}", bad.classify(&file).unwrap_err()),
        "plugin bad: classify a.rs: \"Not A Tag\" is not a tag; use lowercase letters, digits, '-' and '_'"
    );
}

#[test]
fn a_move_that_cannot_be_carried_out_fails_naming_the_plugin() {
    let (file, mut sides) = project("a.rs", "fn a() {}\n", "fn b() {}\n");
    let bad = pipeline(vec![("bad", native::native::<Bad>)]);
    let error = bad.run(&file, &mut sides).unwrap_err();
    assert!(error.downcast_ref::<MutationFailed>().is_some());
    assert_eq!(format!("{error:#}"), "mutation bad: no region 99999");
}

#[test]
fn a_plugin_that_cannot_be_made_is_a_setup_error() {
    let config =
        Config::from_toml("[plugins.bundled.summarize]\nenabled = true\napi_key = ''\n").unwrap();
    // Only meaningful when the environment carries no key.
    if std::env::var_os("GEMINI_API_KEY").is_some() || std::env::var_os("GOOGLE_API_KEY").is_some()
    {
        return;
    }
    let error = Pipeline::from_config(&config.plugins, crate::plugin::native::test_environment())
        .err()
        .expect("a summarizer without a key cannot be made");
    assert_eq!(
        format!("{error:#}"),
        "plugins.bundled.summarize: no API key: set plugins.bundled.summarize.api_key, or GEMINI_API_KEY or GOOGLE_API_KEY in the environment, or turn the summarizer off with plugins.bundled.summarize.enabled = false"
    );
    if std::env::var_os("OPENAI_API_KEY").is_some() {
        return;
    }
    let config = Config::from_toml(
        "[plugins.bundled.summarize]\nenabled = true\nprovider = 'openai'\nmodel = 'm'\napi_key = ''\nendpoint = ''\n",
    )
    .unwrap();
    let error = Pipeline::from_config(&config.plugins, crate::plugin::native::test_environment())
        .err()
        .expect("OpenAI at its default endpoint needs a key");
    assert_eq!(
        format!("{error:#}"),
        "plugins.bundled.summarize: no API key: set plugins.bundled.summarize.api_key, or OPENAI_API_KEY in the environment, or turn the summarizer off with plugins.bundled.summarize.enabled = false"
    );
}

#[test]
fn options_that_do_not_deserialize_are_a_setup_error() {
    let mut pipeline = Pipeline::default();
    let error = pipeline
        .push("bad", json!({"extra": 1}), &native::native::<Bad>)
        .unwrap_err();
    assert_eq!(
        format!("{error:#}"),
        "plugins.bad: invalid options: unknown field `extra`, there are no fields at line 1 column 8"
    );
}

#[test]
fn a_subset_of_bundled_plugins_can_use_shared_query_tags() {
    Config::from_toml("[plugins]\norder = ['bundled.deleted-bodies']\n")
        .unwrap()
        .compile()
        .unwrap();
}
