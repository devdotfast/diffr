//! The plugin host: plugins decide how each diffed file is shown. Nothing in
//! this module is specific to one plugin; the bundled plugins live in
//! `plugins/<name>/`.
//!
//! Every plugin implements the one contract in `wit/plugin.wit`, through the
//! SDK's `Plugin` trait, and diffr loads and runs every plugin the same way.
//! [`config`] reads each enabled entry's folder, embedded or on disk: its
//! `plugin.toml` (name, title, options schema, query files) and, when it has
//! one, its component. [`Pipeline::from_config`] then takes the component
//! ([`wasm`]) or, for a folder without one, the native code registered under
//! the plugin's name ([`native`]), and makes the plugin's one instance for
//! the run from its options. From there a [`Runner`] is a [`Runner`]: each
//! call gets the contract's records, built once per call from the file's
//! manifest entry and its current trees, and the host functions of [`host`].
//!
//! The pipeline for one file: git lists it and its tags, and each plugin's
//! `classify` adds its own; the diff runs with one fold query per language,
//! assembled from the query files of every enabled plugin ([`queries`]); the
//! projection builds the region trees and pairs them; then each enabled
//! plugin's `mutate` runs, in `plugins.order`, on the finished trees,
//! starting with `context`, which collapses unchanged lines far from any
//! change; then `stats.visible` is recounted and the record is written.
//! Queries decide which regions exist and tag them, plugins decide how they
//! start out, and no plugin matches one side to the other: pairing is the
//! projection's.
//!
//! A plugin returns moves. It never edits a tree itself: the SDK's applier
//! carries the moves out, in order, and the next plugin sees the result. A
//! move that cannot be carried out, or a plugin that fails, stops the run
//! (`complete.aborted`).
pub(crate) mod builtin;
pub(crate) mod config;
pub(crate) mod host;
pub(crate) mod native;
pub(crate) mod queries;
pub(crate) mod wasm;

#[cfg(test)]
mod tests;

use crate::pairing::Pairing;
use crate::protocol::{self, FileChange, FileStatus, SourcePos, SourceRange};
use anyhow::{anyhow, Context as _};
use config::{PluginsConfig, COMPONENT_FILE};
use diffr_plugin_sdk::{apply, tree, types};
use host::Host;
use serde_json::Value;
use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

/// One plugin instance: its native code or its component, made once from
/// its options, behind the contract's two calls on a file. Each call gets
/// the host for that call.
pub(crate) trait Runner: Send + Sync {
    fn classify(&self, host: Host, file: &types::FileEntry) -> anyhow::Result<Vec<String>>;

    fn mutate(
        &self,
        host: Host,
        file: &types::FileEntry,
        lhs: Option<&types::Source>,
        rhs: Option<&types::Source>,
    ) -> anyhow::Result<Vec<types::Move>>;
}

/// A plugin stopped the run: its own failure, or a move it asked for
/// that could not be carried out. The stream reports `mutation_failed`.
#[derive(Debug)]
pub(crate) struct MutationFailed(pub(crate) String);

impl fmt::Display for MutationFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mutation {}", self.0)
    }
}

impl std::error::Error for MutationFailed {}

/// Makes a plugin's instance from its options, the entry as a JSON object
/// with defaults filled in, with the host of that call.
pub(crate) type Create<'a> = &'a dyn Fn(Host, &str) -> anyhow::Result<Box<dyn Runner>>;

/// A plugin in the pipeline.
struct Loaded {
    name: Arc<str>,
    runner: Box<dyn Runner>,
}

/// The enabled plugins, in the order they run, and the directory their
/// `git` runs in.
pub(crate) struct Pipeline {
    plugins: Vec<Loaded>,
    workdir: Arc<Path>,
}

impl Default for Pipeline {
    fn default() -> Self {
        Self {
            plugins: Vec::new(),
            workdir: Path::new(".").into(),
        }
    }
}

impl Pipeline {
    /// Load every enabled plugin in `plugins.order` and make its instance. A
    /// folder with a component runs it; any other folder runs the native
    /// code registered under its name. Loading or making a plugin can fail
    /// (a component that does not compile or link, options that do not
    /// deserialize, a summarizer without an API key), which is a setup error
    /// naming the plugin.
    pub(crate) fn from_config(config: &PluginsConfig, workdir: &Path) -> anyhow::Result<Self> {
        let mut pipeline = Self {
            plugins: Vec::new(),
            workdir: workdir.into(),
        };
        let mut engine = None;
        for (name, entry) in config.enabled() {
            let folder = entry.folder();
            let options = Value::Object(entry.options.clone());
            match folder.component() {
                Some(path) => {
                    let engine = match &engine {
                        Some(engine) => engine,
                        None => engine.insert(wasm::engine()?),
                    };
                    pipeline.push(name, options, &|host, options| {
                        wasm::WasmPlugin::load(engine, &path)?.create(host, options)
                    })?
                }
                None => {
                    let create = native::lookup(name).ok_or_else(|| {
                        anyhow!(
                            "plugins.{name}: the plugin folder has no {COMPONENT_FILE}, and diffr has no native plugin of that name"
                        )
                    })?;
                    pipeline.push(name, options, &create)?
                }
            }
        }
        Ok(pipeline)
    }

    /// Make the plugin `name` with `create` from `options` and add it to the
    /// end of the pipeline.
    fn push(&mut self, name: &str, options: Value, create: Create<'_>) -> anyhow::Result<()> {
        let name: Arc<str> = name.into();
        let runner = create(self.host(&name), &options.to_string())
            .with_context(|| format!("plugins.{name}"))?;
        self.plugins.push(Loaded { name, runner });
        Ok(())
    }

    fn host(&self, name: &Arc<str>) -> Host {
        Host {
            name: Arc::clone(name),
            workdir: Arc::clone(&self.workdir),
        }
    }

    /// The file's tags once every plugin has classified it, in order: each
    /// sees the tags the ones before it left. Sorted and deduplicated.
    pub(crate) fn classify(&self, file: &FileChange) -> anyhow::Result<Vec<String>> {
        let mut entry = file_entry(file);
        for plugin in &self.plugins {
            let name = &plugin.name;
            let path = &entry.path;
            let added = plugin
                .runner
                .classify(self.host(&plugin.name), &entry)
                .with_context(|| format!("plugin {name}: classify {path}"))?;
            if let Some(bad) = added.iter().find(|tag| !crate::tags::is_tag(tag)) {
                anyhow::bail!(
                    "plugin {name}: classify {path}: {bad:?} is not a tag; use lowercase letters, digits, '-' and '_'"
                );
            }
            let tags: BTreeSet<String> = entry.tags.drain(..).chain(added).collect();
            entry.tags = tags.into_iter().collect();
        }
        Ok(entry.tags)
    }

    /// Run every plugin on one file's sides, returning the file's own
    /// visibility.
    pub(crate) fn run(
        &self,
        file: &FileChange,
        sides: &mut Pairing<protocol::Source>,
    ) -> anyhow::Result<protocol::Visibility> {
        let entry = file_entry(file);
        let mut trees = match &*sides {
            Pairing::Both { lhs, rhs } => tree::Pairing::Both {
                lhs: to_tree(lhs),
                rhs: to_tree(rhs),
            },
            Pairing::LeftOnly { lhs } => tree::Pairing::LeftOnly { lhs: to_tree(lhs) },
            Pairing::RightOnly { rhs } => tree::Pairing::RightOnly { rhs: to_tree(rhs) },
        };
        let mut visibility = types::Visibility::default();
        for plugin in &self.plugins {
            let started = Instant::now();
            let lhs = trees.lhs().map(tree::Source::to_record);
            let rhs = trees.rhs().map(tree::Source::to_record);
            let moves = plugin
                .runner
                .mutate(self.host(&plugin.name), &entry, lhs.as_ref(), rhs.as_ref())
                .with_context(|| MutationFailed(plugin.name.to_string()))?;
            log::debug!(
                "plugin {}: mutate {} took {:?}",
                plugin.name,
                entry.path,
                started.elapsed()
            );
            apply::apply(moves, &mut trees, &mut visibility)
                .with_context(|| MutationFailed(plugin.name.to_string()))?;
        }
        match (sides, trees) {
            (
                Pairing::Both { lhs, rhs },
                tree::Pairing::Both {
                    lhs: left,
                    rhs: right,
                },
            ) => {
                lhs.regions = from_tree(left.regions);
                rhs.regions = from_tree(right.regions);
            }
            (Pairing::LeftOnly { lhs }, tree::Pairing::LeftOnly { lhs: left }) => {
                lhs.regions = from_tree(left.regions);
            }
            (Pairing::RightOnly { rhs }, tree::Pairing::RightOnly { rhs: right }) => {
                rhs.regions = from_tree(right.regions);
            }
            _ => unreachable!("moves never add or remove a side"),
        }
        Ok(protocol::Visibility {
            collapsed: visibility.collapsed,
            label: visibility.label,
        })
    }
}

/// The contract's record of a manifest entry.
pub(crate) fn file_entry(file: &FileChange) -> types::FileEntry {
    types::FileEntry {
        path: file.file.rhs_or_lhs().path.clone(),
        old_path: file.file.lhs().map(|side| side.path.clone()),
        status: match file.status {
            FileStatus::Added => types::FileStatus::Added,
            FileStatus::Deleted => types::FileStatus::Deleted,
            FileStatus::Modified => types::FileStatus::Modified,
            FileStatus::Renamed => types::FileStatus::Renamed,
            FileStatus::Copied => types::FileStatus::Copied,
            FileStatus::TypeChanged => types::FileStatus::TypeChanged,
        },
        tags: file.tags.clone(),
    }
}

/// One side of the wire as the tree the applier works on.
pub(crate) fn to_tree(side: &protocol::Source) -> tree::Source {
    fn regions(list: &[protocol::Region]) -> Vec<tree::Region> {
        list.iter()
            .map(|region| tree::Region {
                id: region.id,
                fold_state_id: region.fold_state_id,
                range: types::Range {
                    start: types::Position {
                        line: region.range.start.line,
                        column: region.range.start.column,
                    },
                    end: types::Position {
                        line: region.range.end.line,
                        column: region.range.end.column,
                    },
                },
                tags: region.tags.clone(),
                visibility: types::Visibility {
                    collapsed: region.visibility.collapsed,
                    label: region.visibility.label.clone(),
                },
                node: match &region.node {
                    protocol::Node::Leaf {
                        alignment_id,
                        changed,
                    } => tree::Node::Leaf {
                        alignment_id: *alignment_id,
                        changed: changed
                            .iter()
                            .map(|span| types::Span {
                                line: span.line,
                                start_column: span.start_column,
                                end_column: span.end_column,
                            })
                            .collect(),
                    },
                    protocol::Node::Fold { children } => tree::Node::Fold {
                        children: regions(children),
                    },
                },
            })
            .collect()
    }
    tree::Source {
        text: side.text.clone(),
        regions: regions(&side.regions),
    }
}

/// A side's regions back on the wire.
fn from_tree(regions: Vec<tree::Region>) -> Vec<protocol::Region> {
    regions
        .into_iter()
        .map(|region| protocol::Region {
            id: region.id,
            fold_state_id: region.fold_state_id,
            range: SourceRange {
                start: SourcePos {
                    line: region.range.start.line,
                    column: region.range.start.column,
                },
                end: SourcePos {
                    line: region.range.end.line,
                    column: region.range.end.column,
                },
            },
            tags: region.tags,
            visibility: protocol::Visibility {
                collapsed: region.visibility.collapsed,
                label: region.visibility.label,
            },
            node: match region.node {
                tree::Node::Leaf {
                    alignment_id,
                    changed,
                } => protocol::Node::Leaf {
                    alignment_id,
                    changed: changed
                        .into_iter()
                        .map(|span| protocol::Span {
                            line: span.line,
                            start_column: span.start_column,
                            end_column: span.end_column,
                        })
                        .collect(),
                },
                tree::Node::Fold { children } => protocol::Node::Fold {
                    children: from_tree(children),
                },
            },
        })
        .collect()
}
