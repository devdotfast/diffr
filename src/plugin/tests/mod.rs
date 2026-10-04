//! The plugin host and the bundled plugins, run the way the stream runs
//! them: a file projected with the bundled queries, then each plugin's
//! components through a [`Pipeline`].
mod context;
mod deleted_bodies;
mod removed_runs;
mod summarize;
mod test_bodies;

use super::*;
use crate::config::{Config, Params};
use crate::options::DiffOptions;
use crate::pairing::Pairing;
use crate::plugin::cursor::Cursor;
use crate::protocol::{self, project, Diff, FileChange, FileRef, FileStatus, Node, Region, Source};
use serde_json::json;
use std::num::NonZeroUsize;

/// A test side's root. Its id stays clear of the regions' ids and differs
/// between sides, which never share region ids.
pub(crate) fn test_root(regions: Vec<Region>) -> Region {
    let id = 1000 + regions.iter().map(|region| region.id).min().unwrap_or(0);
    Region::root(id, regions)
}
use std::path::Path;

pub(crate) fn walk(regions: &[Region], visit: &mut impl FnMut(&Region)) {
    for region in regions {
        visit(region);
        if let Node::Fold { children, .. } = &region.node {
            walk(children, visit);
        }
    }
}

pub(crate) fn walk_mut(regions: &mut [Region], visit: &mut impl FnMut(&mut Region)) {
    for region in regions {
        visit(region);
        if let Node::Fold { children, .. } = &mut region.node {
            walk_mut(children, visit);
        }
    }
}

pub(crate) fn is_fold(region: &Region) -> bool {
    matches!(region.node, Node::Fold { .. })
}

pub(crate) fn has_tag(region: &Region, tag: &str) -> bool {
    region.tags.iter().any(|own| own == tag)
}

/// Project a two-source comparison with the bundled queries, the way the
/// stream does before the plugins run.
pub(crate) fn project(
    path: &str,
    before: &str,
    after: &str,
) -> (FileChange, Pairing<protocol::Source>) {
    project_with(path, before, after, DiffOptions::default())
}

pub(crate) fn project_with(
    path: &str,
    before: &str,
    after: &str,
    options: DiffOptions,
) -> (FileChange, Pairing<protocol::Source>) {
    let params = Config::from_toml("").unwrap().compile().unwrap();
    project_compiled(path, before, after, &params, options)
}

/// Project with the queries `params` was compiled with.
pub(crate) fn project_compiled(
    path: &str,
    before: &str,
    after: &str,
    params: &Params,
    options: DiffOptions,
) -> (FileChange, Pairing<protocol::Source>) {
    let result = crate::summary::DiffResult::from_sources_with_options(
        path, before, after, params, &options,
    )
    .unwrap();
    let file_ref = FileRef {
        path: path.to_owned(),
        oid: String::new(),
        mode: String::new(),
    };
    let file = FileChange {
        file: Pairing::Both {
            lhs: file_ref.clone(),
            rhs: file_ref,
        },
        status: FileStatus::Modified,
        tags: Vec::new(),
    };
    let diff = project::diff(
        &result,
        project::Inputs {
            file: &file.file,
            sizes: (before.len() as u64, after.len() as u64),
        },
    );
    let Diff::Text { sides, .. } = diff else {
        panic!("text diff expected");
    };
    (file, sides)
}

pub(crate) fn lhs(sides: &Pairing<Source>) -> &Source {
    sides.lhs().expect("a before side")
}

pub(crate) fn rhs(sides: &Pairing<Source>) -> &Source {
    sides.rhs().expect("an after side")
}

/// A pipeline of the bundled plugin `name` alone, configured with
/// `overrides` the way a settings file would.
pub(crate) fn bundled(name: &str, overrides: serde_json::Value) -> Pipeline {
    configured(&format!(
        "[plugins]\norder = ['bundled.{name}']\n[plugins.bundled.{name}]\nenabled = true\n{}",
        options(overrides)
    ))
    .unwrap()
}

/// A one-worker pipeline from a settings file's text.
pub(crate) fn configured(toml: &str) -> anyhow::Result<Pipeline> {
    let config = Config::from_toml(toml)?;
    Pipeline::from_config(&config, Path::new("."), NonZeroUsize::MIN)
}

/// Plugin options as the lines of a settings table.
pub(crate) fn options(options: serde_json::Value) -> String {
    let serde_json::Value::Object(options) = options else {
        panic!("options are an object");
    };
    toml::to_string(&options).unwrap()
}

/// Run `pipeline` on `sides` in place, as the stream does.
pub(crate) fn shape(
    pipeline: &Pipeline,
    file: &FileChange,
    sides: &mut Pairing<Source>,
) -> anyhow::Result<()> {
    *sides = crate::test_runtime().block_on(pipeline.run(file, sides.clone()))?;
    Ok(())
}

/// Run the bundled plugin `name` with `overrides` and carry out its moves.
pub(crate) fn run(
    name: &str,
    overrides: serde_json::Value,
    file: &FileChange,
    sides: &mut Pairing<protocol::Source>,
) {
    shape(&bundled(name, overrides), file, sides).unwrap();
}

/// Run a pipeline and inspect the sides it left, without changing `sides`.
fn edited(
    pipeline: &Pipeline,
    file: &FileChange,
    sides: &Pairing<Source>,
) -> anyhow::Result<Pairing<Source>> {
    crate::test_runtime().block_on(pipeline.run(file, sides.clone()))
}

/// For each `deleted-bodies:function` body on the after side, the first line
/// of the body and the lines of its docstring, as the query relationship identifies it.
fn documented(path: &str, after: &str) -> Vec<(u32, Option<(u32, u32)>)> {
    let (file, sides) = project(path, "", after);
    let source = rhs(&sides);
    let cursor = Cursor::new(file.clone(), sides.clone()).expect("a region");
    let mut bodies = Vec::new();
    walk(source.root.children(), &mut |region| {
        if is_fold(region) && has_tag(region, "deleted-bodies:function") {
            let docstring = cursor
                .related(region.id, "documentation")
                .unwrap()
                .first()
                .map(|id| {
                    let mut lines = None;
                    walk(source.root.children(), &mut |docstring| {
                        if docstring.id == *id {
                            lines = Some(docstring.range.lines());
                        }
                    });
                    let lines = lines.expect("a related region on the same side");
                    (lines.start, lines.end)
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
    let config = Config::default();
    Pipeline::from_config(&config, Path::new("."), NonZeroUsize::MIN).unwrap();
    let made: Vec<&str> = config
        .plugins
        .enabled()
        .map(|(reference, _)| reference.trim_start_matches("bundled."))
        .collect();
    assert_eq!(
        made,
        ["deleted-bodies", "test-bodies", "removed-runs", "context"],
        "the summarizer is off until turned on"
    );
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
    let error = Pipeline::from_config(&config, Path::new("."), NonZeroUsize::MIN)
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
    let error = Pipeline::from_config(&config, Path::new("."), NonZeroUsize::MIN)
        .err()
        .expect("OpenAI at its default endpoint needs a key");
    assert_eq!(
        format!("{error:#}"),
        "plugins.bundled.summarize: no API key: set plugins.bundled.summarize.api_key, or OPENAI_API_KEY in the environment, or turn the summarizer off with plugins.bundled.summarize.enabled = false"
    );
}

/// The manifest can accept an option the component itself rejects: the
/// component's constructor has the last word, and fails setup.
#[test]
fn options_that_do_not_deserialize_are_a_setup_error() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/shape/context/plugin.wasm"),
        dir.path().join("plugin.wasm"),
    )
    .unwrap();
    std::fs::write(
        dir.path().join("plugin.toml"),
        "name = 'context'\ntitle = 'Context'\n[options.extra]\ntype = 'integer'\ntitle = 'Extra'\n",
    )
    .unwrap();
    let error = configured(&format!(
        "[plugins]\norder = ['external.context']\n[plugins.external.context]\npath = {:?}\nextra = 1\n",
        dir.path()
    ))
    .err()
    .expect("unknown option rejected");
    let error = format!("{error:#}");
    assert!(error.contains("plugins.external.context"), "{error}");
    assert!(error.contains("unknown field `extra`"), "{error}");
}

#[test]
fn external_plugins_require_a_component() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("plugin.toml"),
        "name = 'context'\ntitle = 'External context'\n",
    )
    .unwrap();
    let config = Config::from_toml_in(
        "[plugins]\norder = ['external.context']\n[plugins.external.context]\npath = '.'\n",
        dir.path(),
    )
    .unwrap();
    let error = Pipeline::from_config(&config, dir.path(), NonZeroUsize::MIN)
        .err()
        .unwrap();
    let error = format!("{error:#}");
    assert!(error.contains("plugin.wasm"), "{error}");
    assert!(error.contains("plugins.external.context"), "{error}");
}

#[test]
fn a_subset_of_bundled_plugins_can_use_shared_query_tags() {
    Config::from_toml("[plugins]\norder = ['bundled.deleted-bodies']\n")
        .unwrap()
        .compile()
        .unwrap();
}

#[test]
fn documentation_relationship_comes_from_query_captures_not_distance() {
    let parameters = (0..16)
        .map(|i| format!("    arg{i}: u32,\n"))
        .collect::<String>();
    let source = format!("/// This belongs to f.\n/// Even with a long signature.\nfn f(\n{parameters}) {{\n    first();\n    second();\n}}\n");
    assert_eq!(documented("long.rs", &source), [(20, Some((0, 2)))]);
}

/// A component's exports decide its kind: a shape plugin is no classifier,
/// and the classifier is no shape plugin.
#[test]
fn a_plugin_configured_as_the_wrong_kind_is_a_setup_error() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let config = Config::from_toml(&format!(
        "[classifier]\npath = {:?}\n",
        root.join("plugins/shape/context")
    ))
    .unwrap();
    let error = Classifier::from_config(&config, Path::new("."))
        .err()
        .expect("a shape plugin is not a classifier");
    assert!(
        format!("{error:#}").contains("not a classifier"),
        "{error:#}"
    );
    let error = configured(&format!(
        "[plugins]\norder = ['external.classify']\n[plugins.external.classify]\npath = {:?}\n",
        root.join("plugins/classify")
    ))
    .err()
    .expect("the classifier is not a shape plugin");
    assert!(
        format!("{error:#}").contains("not a shape plugin"),
        "{error:#}"
    );
}
