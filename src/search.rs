//! Search owns pinned-blob indexing and hit hydration. Ordinary diff fast paths
//! are unchanged; plugin inputs use the shared source/region schema.
use crate::{
    config::Config,
    git,
    pairing::{Comparison, Pairing},
    plugin::Pipeline,
    protocol::{self, FileChange, FileRef, FileStatus, Node, Region, Source, Span},
    summary::DiffResult,
};
use anyhow::{anyhow, bail, ensure, Context};
use git2::{Oid, Repository};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};
pub mod store;
use store::{DiffKey, Store, StoredDiff};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Worktree {
    pub commit_id: String,
    pub path: PathBuf,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Scope {
    pub repo: PathBuf,
    pub base_worktree: Worktree,
    pub head_worktree: Worktree,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hit {
    pub file: PathBuf,
    pub lines: Vec<HitLine>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HitLine {
    pub line: u32,
    pub text: String,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Display {
    Both,
    Lhs,
    Rhs,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchResult {
    display: Display,
    scope: Scope,
    #[serde(flatten)]
    comparison: Comparison<FileRef, Source>,
}
/// Shared computed-diff index. Comparison and plugin state belong to sessions.
pub struct Index<S: Store + ?Sized> {
    store: Arc<S>,
}

/// One pinned comparison and its analysis pipeline, using a shared index.
pub struct Session<S: Store + ?Sized> {
    index: Arc<Index<S>>,
    scope: Scope,
    params: Arc<crate::config::Params>,
    pipeline: Pipeline,
    manifest: Vec<FileChange>,
    analysis: String,
}

/// Per-call plugin overrides affect analysis, never the shared store lifetime.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Options {
    plugins: Option<crate::plugin::config::PluginsConfig>,
}

