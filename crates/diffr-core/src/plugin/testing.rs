//! What tests of plugins share: projecting two sources into the trees
//! plugins see, and pipelines of one bundled plugin. A crate that tests
//! against the pipeline gets it with the `test-support` feature.
use super::config::ComponentSource;
use super::native::{self, BundledNatively};
use super::{
    builtin, file_entry, from_tree, source_sides, to_tree, Components, Instantiate, Pipeline,
};
use crate::config::Config;
use crate::pairing::Pairing;
use crate::params::{DiffOptions, Params};
use crate::protocol::{self, project, Diff, FileChange, FileRef, FileStatus};
use diffr_plugin_sdk::{tree, Move};

/// Project a two-source comparison with the bundled queries, the way the
/// stream does before the plugins run.
pub fn project(path: &str, before: &str, after: &str) -> (FileChange, Pairing<protocol::Source>) {
    project_with(path, before, after, DiffOptions::default())
}

pub fn project_with(
    path: &str,
    before: &str,
    after: &str,
    options: DiffOptions,
) -> (FileChange, Pairing<protocol::Source>) {
    let params = Config::from_toml("").unwrap().compile().unwrap();
    project_compiled(path, before, after, &params, options)
}

/// Project with the queries `params` was compiled with.
pub fn project_compiled(
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
            syntax: (Vec::new(), Vec::new()),
        },
    );
    let Diff::Text { sides, .. } = diff else {
        panic!("text diff expected");
    };
    (file, sides)
}

/// A manifest entry for `path` on the sides `sides` names.
pub fn manifest<T>(path: &str, sides: &tree::Pairing<T>, status: FileStatus) -> FileChange {
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
pub fn trees(sides: &Pairing<protocol::Source>) -> tree::Pairing<tree::Source> {
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
pub fn wire(sides: tree::Pairing<tree::Source>) -> Pairing<protocol::Source> {
    let source = |side: tree::Source| protocol::Source {
        text: side.text,
        syntax: Vec::new(),
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

pub fn lhs(sides: &tree::Pairing<tree::Source>) -> &tree::Source {
    sides.lhs().expect("a before side")
}

pub fn rhs(sides: &tree::Pairing<tree::Source>) -> &tree::Source {
    sides.rhs().expect("an after side")
}

/// A pipeline of the bundled plugin `name` alone, made with its defaults and
/// `overrides`, a component run natively ([`BundledNatively`]).
pub fn bundled(name: &str, overrides: serde_json::Value) -> Pipeline {
    bundled_with(name, overrides, &BundledNatively)
}

/// A pipeline of the bundled plugin `name` alone, made with its defaults and
/// `overrides`, a component loaded by `components`.
pub fn bundled_with(
    name: &str,
    overrides: serde_json::Value,
    components: &dyn Components,
) -> Pipeline {
    let serde_json::Value::Object(mut options) = overrides else {
        panic!("overrides are an object");
    };
    builtin::manifest(name)
        .expect("a bundled plugin")
        .fill_defaults(&mut options);
    let mut pipeline = Pipeline::default();
    let create: Instantiate = match builtin::component(name) {
        Some(bytes) => components
            .load(&ComponentSource::Bundled(bytes))
            .expect("the bundled component loads"),
        None => {
            let registration = native::lookup(name).unwrap().expect("native code");
            Box::new(move |host, options| native::registered(registration, host, options))
        }
    };
    pipeline
        .push(name, serde_json::Value::Object(options), &*create)
        .unwrap();
    pipeline
}

/// Run the bundled plugin `name` with `overrides` and carry out its moves.
pub fn run(
    name: &str,
    overrides: serde_json::Value,
    file: &FileChange,
    sides: &mut Pairing<protocol::Source>,
) {
    bundled(name, overrides).run(file, sides).unwrap();
}

/// Run the bundled plugin `name` with `overrides` on trees built by hand,
/// and carry out its moves.
pub fn run_trees(
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
pub fn moves(
    pipeline: &Pipeline,
    file: &FileChange,
    sides: &Pairing<protocol::Source>,
) -> anyhow::Result<Vec<Move>> {
    let [plugin] = &pipeline.plugins[..] else {
        panic!("one plugin");
    };
    let records = source_sides(&trees(sides));
    plugin
        .runner
        .mutate(pipeline.host(&plugin.name), &file_entry(file), &records)
}
