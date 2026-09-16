//! The plugin host and the bundled plugins, run the way the stream runs
//! them: a file projected with the bundled queries, then each plugin's
//! native code through a [`Pipeline`].
mod context;
mod deleted_bodies;
mod group;
mod hide_files;
mod removed_runs;
mod test_bodies;

use super::*;
use crate::config::{Config, Params};
use crate::options::DiffOptions;
use crate::protocol::{project, Diff, FileRef};
use diffr_plugin_sdk::tree::{docstring_of, has_tag, is_fold, walk};
use diffr_plugin_sdk::{FileEntry, Move, Plugin};
use serde_json::json;

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
        path,
        before,
        after,
        params,
        &crate::options::DisplayOptions::default(),
        &options,
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

/// A manifest entry for `path` on the sides `sides` names.
pub(crate) fn manifest<T>(path: &str, sides: &tree::Pairing<T>, status: FileStatus) -> FileChange {
    let file_ref = || FileRef {
        path: path.to_owned(),
        oid: String::new(),
        mode: String::new(),
    };
    FileChange {
        file: match sides {
            tree::Pairing::Both { .. } => Pairing::Both {
                lhs: file_ref(),
                rhs: file_ref(),
            },
            tree::Pairing::LeftOnly { .. } => Pairing::LeftOnly { lhs: file_ref() },
            tree::Pairing::RightOnly { .. } => Pairing::RightOnly { rhs: file_ref() },
        },
        status,
        tags: Vec::new(),
    }
}

/// The wire's sides as the trees plugins read.
pub(crate) fn trees(sides: &Pairing<protocol::Source>) -> tree::Pairing<tree::Source> {
    match sides {
        Pairing::Both { lhs, rhs } => tree::Pairing::Both {
            lhs: to_tree(lhs),
            rhs: to_tree(rhs),
        },
        Pairing::LeftOnly { lhs } => tree::Pairing::LeftOnly { lhs: to_tree(lhs) },
        Pairing::RightOnly { rhs } => tree::Pairing::RightOnly { rhs: to_tree(rhs) },
    }
}

/// Trees built by hand, as the wire's sides.
pub(crate) fn wire(sides: tree::Pairing<tree::Source>) -> Pairing<protocol::Source> {
    let source = |side: tree::Source| protocol::Source {
        text: side.text,
        regions: from_tree(side.regions),
    };
    match sides {
        tree::Pairing::Both { lhs, rhs } => Pairing::Both {
            lhs: source(lhs),
            rhs: source(rhs),
        },
        tree::Pairing::LeftOnly { lhs } => Pairing::LeftOnly { lhs: source(lhs) },
        tree::Pairing::RightOnly { rhs } => Pairing::RightOnly { rhs: source(rhs) },
    }
}

pub(crate) fn lhs(sides: &tree::Pairing<tree::Source>) -> &tree::Source {
    sides.lhs().expect("a before side")
}

pub(crate) fn rhs(sides: &tree::Pairing<tree::Source>) -> &tree::Source {
    sides.rhs().expect("an after side")
}

/// A pipeline of the bundled plugin `name` alone, made with its defaults and
/// `overrides`.
pub(crate) fn bundled(name: &str, overrides: serde_json::Value) -> Pipeline {
    let mut options = builtin::manifest(name)
        .expect("a bundled plugin")
        .defaults();
    let serde_json::Value::Object(overrides) = overrides else {
        panic!("overrides are an object");
    };
    options.extend(overrides);
    let mut pipeline = Pipeline::default();
    pipeline
        .push(
            name,
            serde_json::Value::Object(options),
            &native::lookup(name).expect("native code"),
        )
        .unwrap();
    pipeline
}

/// Run the bundled plugin `name` with `overrides` and carry out its moves.
pub(crate) fn run(
    name: &str,
    overrides: serde_json::Value,
    file: &FileChange,
    sides: &mut Pairing<protocol::Source>,
) {
    bundled(name, overrides).run(file, sides).unwrap();
}

/// Run the bundled plugin `name` with `overrides` on trees built by hand,
/// and carry out its moves.
pub(crate) fn run_trees(
    name: &str,
    overrides: serde_json::Value,
    file: &FileChange,
    sides: &mut tree::Pairing<tree::Source>,
) {
    let mut wired = wire(sides.clone());
    run(name, overrides, file, &mut wired);
    *sides = trees(&wired);
}

/// The moves the only plugin of `pipeline` asks for, not carried out.
pub(crate) fn moves(
    pipeline: &Pipeline,
    file: &FileChange,
    sides: &Pairing<protocol::Source>,
) -> anyhow::Result<Vec<Move>> {
    let [plugin] = &pipeline.plugins[..] else {
        panic!("one plugin");
    };
    let sides = trees(sides);
    let (lhs, rhs) = (
        sides.lhs().map(tree::Source::to_record),
        sides.rhs().map(tree::Source::to_record),
    );
    plugin.runner.mutate(
        pipeline.host(&plugin.name, no_head()),
        &file_entry(file),
        lhs.as_ref(),
        rhs.as_ref(),
    )
}

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
    let pipeline = Pipeline::from_config(&PluginsConfig::default(), Path::new(".")).unwrap();
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
        ]
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

    fn mutate(
        &self,
        _: &FileEntry,
        _: Option<&types::Source>,
        _: Option<&types::Source>,
    ) -> anyhow::Result<Vec<Move>> {
        Ok(vec![Move::SetCollapsed {
            region: 99_999,
            collapsed: true,
        }])
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

    fn mutate(
        &self,
        _: &FileEntry,
        _: Option<&types::Source>,
        _: Option<&types::Source>,
    ) -> anyhow::Result<Vec<Move>> {
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

    fn mutate(
        &self,
        _: &FileEntry,
        _: Option<&types::Source>,
        _: Option<&types::Source>,
    ) -> anyhow::Result<Vec<Move>> {
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

    fn mutate(
        &self,
        _: &FileEntry,
        _: Option<&types::Source>,
        _: Option<&types::Source>,
    ) -> anyhow::Result<Vec<Move>> {
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
    let head = no_head();
    let tagging = pipeline(vec![
        ("first", native::native::<TagsAZ>),
        ("second", native::native::<TagsB>),
    ]);
    assert_eq!(tagging.classify(&file, &head).unwrap(), ["a", "b", "z"]);
    let bad = pipeline(vec![("bad", native::native::<NotATag>)]);
    assert_eq!(
        format!("{:#}", bad.classify(&file, &head).unwrap_err()),
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
