//! The plugin host: plugins decide how each diffed file is shown. Nothing in
//! this module is specific to one plugin; the bundled plugins live in
//! `plugins/<name>/`.
//!
//! Every plugin implements the one contract in `wit/plugin.wit`, through the
//! SDK's `Plugin` trait, and diffr loads and runs every plugin the same way.
//! [`config`] reads each enabled entry's folder, embedded or on disk: its
//! `plugin.toml` (name, title, options schema) and, when it has
//! one, its component. [`Pipeline::from_config`] then takes the component
//! ([`wasm`]) or the bundled native implementation registered under
//! the plugin's name ([`native`]), and makes the plugin's instance pool for
//! the run from its options. From there a [`Runner`] is a [`Runner`]: each
//! call gets the contract's records, built once per call from the file's
//! manifest entry and its current trees, and the host functions of [`host`].
//!
//! The pipeline for one file: git lists it and its tags, and each plugin's
//! `classify` adds its own; the diff runs with one fold query per language,
//! assembled from the source text returned by every enabled plugin ([`queries`]); the
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
mod pool;
pub(crate) mod queries;
pub(crate) mod wasm;

#[cfg(test)]
mod tests;

use crate::pairing::Pairing;
use crate::protocol::{self, FileChange, FileStatus, SourcePos, SourceRange};
use anyhow::{anyhow, Context as _};
use config::PluginsConfig;
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
    fn enrich(
        &self,
        _host: Host,
        _file: &types::FileEntry,
        _sides: &types::SourceSides,
    ) -> anyhow::Result<Vec<types::Annotation>> {
        Ok(Vec::new())
    }

    fn queries(&self, host: Host) -> anyhow::Result<Vec<types::QuerySource>>;

    fn classify(&self, host: Host, file: &types::FileEntry) -> anyhow::Result<Vec<String>>;

    fn mutate(
        &self,
        host: Host,
        file: &types::FileEntry,
        sides: &types::SourceSides,
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
    /// external entry loads its component. A bundled entry loads its embedded
    /// component or the native implementation selected by package metadata. Loading or making a plugin can fail
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
                    let plugin = wasm::WasmPlugin::load(engine, &path)
                        .with_context(|| format!("plugins.{name}"))?;
                    pipeline.push_instances(
                        name,
                        options,
                        entry
                            .instances
                            .unwrap_or(if folder.manifest.parallel { 4 } else { 1 }),
                        &|host, options| plugin.create(host, options),
                    )?
                }
                None => {
                    let create = native::lookup(&folder.manifest.name)?.ok_or_else(|| {
                        anyhow!(
                            "plugins.{name}: no native implementation is registered for this bundled plugin"
                        )
                    })?;
                    pipeline.push_instances(
                        name,
                        options,
                        entry
                            .instances
                            .unwrap_or(if folder.manifest.parallel { 4 } else { 1 }),
                        &|host, options| native::registered(create, host, options),
                    )?
                }
            }
        }
        Ok(pipeline)
    }

    /// Make the plugin `name` with `create` from `options` and add it to the
    /// end of the pipeline.
    #[cfg(test)]
    fn push(&mut self, name: &str, options: Value, create: Create<'_>) -> anyhow::Result<()> {
        self.push_instances(name, options, 1, create)
    }

    fn push_instances(
        &mut self,
        name: &str,
        options: Value,
        instances: usize,
        create: Create<'_>,
    ) -> anyhow::Result<()> {
        let reference = name;
        let name: Arc<str> = name.split_once('.').map_or(name, |(_, name)| name).into();
        let runners = (0..instances)
            .map(|_| {
                create(self.host(&name), &options.to_string())
                    .with_context(|| format!("plugins.{reference}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let initial = if instances > 1 {
            Some(
                create(self.host(&name), &options.to_string())
                    .with_context(|| format!("plugins.{reference}"))?,
            )
        } else {
            None
        };
        let runner = Box::new(pool::Pool::with_initial(runners, initial));
        self.plugins.push(Loaded { name, runner });
        Ok(())
    }

    /// Collect raw sources from the same instances that classify and mutate.
    pub(crate) fn queries(&self) -> anyhow::Result<Vec<(String, Vec<types::QuerySource>)>> {
        self.plugins
            .iter()
            .map(|plugin| {
                let sources = plugin
                    .runner
                    .queries(self.host(&plugin.name))
                    .with_context(|| format!("plugin {}: queries", plugin.name))?;
                Ok((plugin.name.to_string(), sources))
            })
            .collect()
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
            let path = entry.path();
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

    /// Compatibility path: return the fully enriched file in one operation.
    pub(crate) fn run(
        &self,
        file: &FileChange,
        sides: &mut Pairing<protocol::Source>,
    ) -> anyhow::Result<protocol::Visibility> {
        let visibility = self.prepare(file, sides)?;
        let annotations = self.enrich(file, sides)?;
        Self::apply_annotations(sides, &annotations)?;
        Ok(visibility)
    }

    /// Deferred plugins may only attach labels to existing regions.
    pub(crate) fn enrich(
        &self,
        file: &FileChange,
        sides: &Pairing<protocol::Source>,
    ) -> anyhow::Result<Vec<protocol::Annotation>> {
        let trees = match sides {
            Pairing::Both { lhs, rhs } => tree::Pairing::Both {
                lhs: to_tree(lhs),
                rhs: to_tree(rhs),
            },
            Pairing::LeftOnly { lhs } => tree::Pairing::LeftOnly { lhs: to_tree(lhs) },
            Pairing::RightOnly { rhs } => tree::Pairing::RightOnly { rhs: to_tree(rhs) },
        };
        let records = source_sides(&trees);
        let entry = file_entry(file);
        let mut annotations = Vec::new();
        for plugin in &self.plugins {
            let labels = plugin
                .runner
                .enrich(self.host(&plugin.name), &entry, &records)
                .with_context(|| MutationFailed(plugin.name.to_string()))?;
            annotations.extend(labels.into_iter().map(|label| protocol::Annotation {
                region_id: label.region_id,
                label: label.label,
            }));
        }
        // Validate the complete batch before it can leave the host.
        Self::apply_annotations(&mut sides.clone(), &annotations)?;
        Ok(annotations)
    }

    pub(crate) fn apply_annotations(
        sides: &mut Pairing<protocol::Source>,
        annotations: &[protocol::Annotation],
    ) -> anyhow::Result<()> {
        fn find(regions: &mut [protocol::Region], id: u32) -> Option<&mut protocol::Region> {
            for region in regions {
                if region.id == id {
                    return Some(region);
                }
                if let protocol::Node::Fold { children } = &mut region.node {
                    if let Some(found) = find(children, id) {
                        return Some(found);
                    }
                }
            }
            None
        }
        for annotation in annotations {
            let region = match sides {
                Pairing::Both { lhs, rhs } => find(&mut lhs.regions, annotation.region_id)
                    .or_else(|| find(&mut rhs.regions, annotation.region_id)),
                Pairing::LeftOnly { lhs } => find(&mut lhs.regions, annotation.region_id),
                Pairing::RightOnly { rhs } => find(&mut rhs.regions, annotation.region_id),
            }
            .ok_or_else(|| {
                anyhow!(
                    "annotation refers to missing region {}",
                    annotation.region_id
                )
            })?;
            region.visibility.label = annotation.label.clone();
        }
        Ok(())
    }

    /// Run every plugin on one file's sides, returning the file's own
    /// visibility.
    pub(crate) fn prepare(
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
            let records = source_sides(&trees);
            let moves = plugin
                .runner
                .mutate(self.host(&plugin.name), &entry, &records)
                .with_context(|| MutationFailed(plugin.name.to_string()))?;
            log::debug!(
                "plugin {}: mutate {} took {:?}",
                plugin.name,
                entry.path(),
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
    let file_ref = |side: &protocol::FileRef| types::FileRef {
        path: side.path.clone(),
        oid: side.oid.clone(),
        mode: side.mode.clone(),
    };
    types::FileEntry {
        file: match &file.file {
            Pairing::Both { lhs, rhs } => types::FileSides::Both((file_ref(lhs), file_ref(rhs))),
            Pairing::LeftOnly { lhs } => types::FileSides::LeftOnly(file_ref(lhs)),
            Pairing::RightOnly { rhs } => types::FileSides::RightOnly(file_ref(rhs)),
        },
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

/// The trees a plugin is given, as the contract's records.
fn source_sides(trees: &tree::Pairing<tree::Source>) -> types::SourceSides {
    match trees {
        tree::Pairing::Both { lhs, rhs } => {
            types::SourceSides::Both((lhs.to_record(), rhs.to_record()))
        }
        tree::Pairing::LeftOnly { lhs } => types::SourceSides::LeftOnly(lhs.to_record()),
        tree::Pairing::RightOnly { rhs } => types::SourceSides::RightOnly(rhs.to_record()),
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
