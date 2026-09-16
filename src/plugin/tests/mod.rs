//! The plugin host and the bundled plugins, run the way the stream runs
//! them: a file projected with the bundled queries, then each plugin's
//! native code through a [`Pipeline`].
mod context;

use super::*;
use crate::config::{Config, Params};
use crate::options::DiffOptions;
use crate::protocol::{project, Diff, FileRef};
use diffr_plugin_sdk::tree::{has_tag, is_fold, walk};
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

#[test]
fn the_default_pipeline_makes_every_plugin_that_is_on() {
    let pipeline = Pipeline::from_config(&PluginsConfig::default(), Path::new(".")).unwrap();
    let made: Vec<&str> = pipeline
        .plugins
        .iter()
        .map(|plugin| &*plugin.name)
        .collect();
    assert_eq!(made, ["context"]);
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
