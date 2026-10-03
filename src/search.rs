//! Hydrate grep hits against pinned commits and run the search plugins.
use crate::{
    config::{Config, Params},
    git,
    options::DiffOptions,
    pairing::Pairing,
    plugin::{config::ShapeConfig, Classifier, Pipeline},
    present::present,
    protocol::{project, Diff, FileChange, FileRef, FileStatus, Node, Region, Source, Span},
    summary::{DiffResult, FallbackCause},
    tags::GENERATED,
};
use anyhow::{anyhow, bail, ensure, Context};
use gix::{ObjectId as Oid, Repository};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    num::NonZeroUsize,
    path::{Component, Path, PathBuf},
    slice,
    sync::Mutex,
};
pub(crate) mod store;
use store::{DiffKey, FileStore, StoredDiff};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Scope {
    pub(crate) repo: PathBuf,
    pub(crate) base: String,
    pub(crate) head: String,
}
impl Scope {
    /// Resolve revisions to commit IDs once.
    pub(crate) fn resolve(repo: &Path, base: &str, head: &str) -> anyhow::Result<Self> {
        let repo = gix::discover(repo)?;
        Ok(Self {
            repo: repo.workdir().unwrap_or(repo.git_dir()).canonicalize()?,
            base: commit(&repo, base)?.to_string(),
            head: commit(&repo, head)?.to_string(),
        })
    }
}
pub(crate) struct Hit {
    pub(crate) revision: String,
    pub(crate) file: PathBuf,
    pub(crate) lines: Vec<HitLine>,
}
pub(crate) struct HitLine {
    pub(crate) line: u32,
    pub(crate) text: String,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Display {
    Both,
    Lhs,
    Rhs,
}
/// The one plugin search runs by default.
const CONTEXT: &str = "bundled.context";

/// One file's search result: the stream's file record (`file` and `diff`),
/// with the side to show and its scope. An unchanged file has two identical
/// sides.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SearchResult {
    display: Display,
    pub(crate) scope: Scope,
    file: Pairing<FileRef>,
    diff: Diff,
}
impl SearchResult {
    /// The text diff's sources, which must exist on the file's sides.
    fn sources(&self) -> anyhow::Result<&Pairing<Source>> {
        let Diff::Text { sides, .. } = &self.diff else {
            bail!("search results are text diffs");
        };
        sides_agree(&self.file, sides)?;
        Ok(sides)
    }
    fn sources_mut(&mut self) -> anyhow::Result<&mut Pairing<Source>> {
        let Diff::Text { sides, .. } = &mut self.diff else {
            bail!("search results are text diffs");
        };
        sides_agree(&self.file, sides)?;
        Ok(sides)
    }
}
fn sides_agree(file: &Pairing<FileRef>, sources: &Pairing<Source>) -> anyhow::Result<()> {
    ensure!(
        file.lhs().is_some() == sources.lhs().is_some()
            && file.rhs().is_some() == sources.rhs().is_some(),
        "file and source sides must agree"
    );
    Ok(())
}

/// Two pinned commits and their plugin pipeline.
pub(crate) struct Session {
    store: FileStore,
    scope: Scope,
    params: Params,
    pipeline: Pipeline,
    manifest: Vec<(FileChange, Option<String>)>,
    classifier: Mutex<Classifier>,
    analysis: DiffKey,
}

/// Per-call plugin overrides.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Options {
    plugins: Option<ShapeConfig>,
}

/// Create a session for `scope`, caching computed diffs under the configured path.
pub(crate) fn configured_session(scope: Scope, options: Options) -> anyhow::Result<Session> {
    let mut config = Config::load()?;
    let store = FileStore::open(scope.repo.join(&config.storage.path))?;
    let defaults = search_plugins(&config, &scope.repo)?;
    // Always load context queries, even if ordinary diffs disable context.
    let shape = &mut config.plugins.shape;
    shape
        .entries
        .insert(CONTEXT.to_owned(), defaults.entries[CONTEXT].clone());
    if !shape.order.iter().any(|name| name == CONTEXT) {
        shape.order.insert(0, CONTEXT.to_owned());
    }
    let mut processing = config.clone();
    processing.plugins.shape = options.plugins.unwrap_or(defaults);
    processing.plugins.shape.resolve(&scope.repo)?;
    let pipeline = Pipeline::from_config(&processing, &scope.repo, NonZeroUsize::MIN)?;
    validate(&scope)?;
    let queries = config.plugins.shape.queries()?;
    let resolved = crate::plugin::queries::sources(&queries)?;
    let analysis = DiffKey::new(&(
        serde_json::to_value(config.diff)?,
        serde_json::to_value(&config.plugins)?,
        resolved,
    ))?;
    let params = config.compile_queries(queries)?;
    let listing = git::list(
        &scope.repo,
        git::Comparison {
            before: git::Operand::revision(&scope.base),
            after: git::Operand::revision(&scope.head),
        },
        &git::FileParams::default(),
    )
    .map_err(|e| anyhow!("{e}"))?;
    let entries: Vec<_> = listing
        .files
        .iter()
        .map(|f| f.change.manifest_entry())
        .collect();
    let mut classifier = Classifier::from_config(&config, &scope.repo)?;
    let classified = classifier.classify(&entries)?;
    let manifest = entries
        .into_iter()
        .zip(classified)
        .map(|(mut entry, classification)| {
            entry.tags = classification.tags;
            (entry, classification.hidden)
        })
        .collect();
    Ok(Session {
        store,
        scope,
        params,
        pipeline,
        manifest,
        classifier: Mutex::new(classifier),
        analysis,
    })
}

fn search_plugins(config: &Config, repo: &Path) -> anyhow::Result<ShapeConfig> {
    let mut plugins = ShapeConfig {
        order: vec![CONTEXT.into()],
        ..Default::default()
    };
    plugins
        .entries
        .retain(|name, _| plugins.order.contains(name));
    let context = plugins
        .entries
        .get_mut(CONTEXT)
        .context("the bundled context plugin is missing")?;
    if let Some(lines) = config
        .plugins
        .shape
        .entries
        .get(CONTEXT)
        .and_then(|entry| entry.options.get("lines"))
    {
        context.options.insert("lines".into(), lines.clone());
    }
    plugins.resolve(repo)?;
    Ok(plugins)
}

impl Session {
    /// Mark each hit on its file's diff at the pinned commits, then shape every
    /// file around its hits with the search plugins.
    pub(crate) fn search(
        &self,
        hits: Vec<Hit>,
        runtime: &tokio::runtime::Runtime,
    ) -> anyhow::Result<Vec<SearchResult>> {
        let scope = &self.scope;
        let repo = gix::open(&scope.repo)?;
        let base = oid(&scope.base)?;
        let head = oid(&scope.head)?;
        let mut revisions =
            BTreeMap::from([(scope.base.clone(), base), (scope.head.clone(), head)]);
        let mut selected: BTreeMap<String, (StoredDiff, SearchResult)> = BTreeMap::new();
        for hit in hits {
            ensure!(
                !hit.file.as_os_str().is_empty()
                    && hit
                        .file
                        .components()
                        .all(|c| matches!(c, Component::Normal(_))),
                "hit paths must be repository-relative without parent traversal"
            );
            let path = hit.file.to_str().context("non-UTF8 hit path")?;
            let oid = match revisions.get(&hit.revision) {
                Some(oid) => *oid,
                None => {
                    let oid = commit(&repo, &hit.revision)?;
                    revisions.insert(hit.revision.clone(), oid);
                    oid
                }
            };
            let right = oid == head;
            ensure!(
                right || oid == base,
                "hit revision is outside scoped commits"
            );
            let cached = self.resolve(right, path)?;
            let (_, result) = selected.entry(key(&cached.entry.file)).or_insert_with(|| {
                let result = SearchResult {
                    display: Display::Both,
                    scope: scope.clone(),
                    file: cached.entry.file.clone(),
                    diff: Diff::Text {
                        sides: cached.sources.clone(),
                        stats: cached.stats.clone(),
                    },
                };
                (cached, result)
            });
            let output = side_mut(result.sources_mut()?, right).context("hit side absent")?;
            let mut spans = Vec::new();
            for line in hit.lines {
                ensure!(line.line > 0, "hit line numbers are 1-based");
                let text = output
                    .text
                    .split_terminator('\n')
                    .nth((line.line - 1) as usize)
                    .context("hit line is out of bounds")?;
                let text = text.strip_suffix('\r').unwrap_or(text);
                ensure!(
                    text == line.text,
                    "hit text differs from pinned source at {path}:{}",
                    line.line
                );
                spans.push(Span {
                    line: line.line - 1,
                    start_column: 0,
                    end_column: u32::try_from(text.len())?,
                });
            }
            add_highlights(slice::from_mut(&mut output.root), spans);
        }
        let mut results = Vec::new();
        for (cached, mut result) in selected.into_values() {
            // A hidden file runs no plugin, so its hits stay on show.
            if cached.hidden.is_none() {
                result.diff = runtime.block_on(present(None, result.diff, async |sides| {
                    self.pipeline.run(&cached.entry, sides).await
                }))?;
            }
            results.push(result);
        }
        Ok(results)
    }

    fn resolve(&self, right: bool, path: &str) -> anyhow::Result<StoredDiff> {
        let scope = &self.scope;
        let (entry, hidden) = if let Some(entry) = self.manifest.iter().find(|(entry, _)| {
            sides(&entry.file)
                .iter()
                .any(|(side, file)| *side == right && file.path == path)
        }) {
            entry.clone()
        } else {
            let repo = gix::open(&scope.repo)?;
            let file = |commit: &str| -> anyhow::Result<FileRef> {
                let tree = repo.find_commit(oid(commit)?)?.tree()?;
                let entry = tree
                    .lookup_entry_by_path(path)?
                    .context("hit does not resolve to a file in that commit")?;
                ensure!(
                    entry.mode().is_blob() && !entry.mode().is_link(),
                    "search requires regular text files"
                );
                Ok(FileRef {
                    path: path.to_owned(),
                    oid: entry.object_id().to_string(),
                    mode: format!("{:o}", entry.mode().value()),
                })
            };
            let lhs = file(&scope.base)?;
            let rhs = file(&scope.head)?;
            ensure!(
                lhs.oid == rhs.oid,
                "changed file missing from comparison manifest"
            );
            let mut entry = FileChange {
                file: Pairing::Both { lhs, rhs },
                status: FileStatus::Unchanged,
                tags: Vec::new(),
            };
            let classification = self
                .classifier
                .lock()
                .map_err(|_| anyhow!("classifier lock poisoned"))?
                .classify(slice::from_ref(&entry))?
                .remove(0);
            entry.tags = classification.tags;
            (entry, classification.hidden)
        };
        let cache_key = DiffKey::new(&(&self.analysis, path, &entry, &hidden))?;
        if let Some(diff) = self.store.get(&cache_key)? {
            return Ok(diff);
        }
        let repo = gix::open(&scope.repo)?;
        let read = |file: &FileRef| -> anyhow::Result<String> {
            ensure!(
                file.mode == "100644" || file.mode == "100755",
                "search requires regular text files"
            );
            let blob = repo.find_blob(oid(&file.oid)?)?;
            ensure!(!blob.data.contains(&0), "search requires text files");
            Ok(std::str::from_utf8(&blob.data)?.to_owned())
        };
        let mut text = [String::new(), String::new()];
        for (right, file) in sides(&entry.file) {
            text[usize::from(right)] = read(file)?;
        }
        let options = DiffOptions {
            by_line: if entry.tags.iter().any(|tag| tag == GENERATED) {
                Some(FallbackCause::Generated)
            } else if hidden.is_some() {
                Some(FallbackCause::Hidden)
            } else {
                None
            },
            syntax: true,
            ..self.params.diff.options(false)
        };
        let diff = DiffResult::from_sources_with_context(
            path,
            &text[0],
            &text[1],
            &self.params,
            &options,
        )?;
        let Diff::Text { sides, stats } = project::diff(
            &diff,
            project::Inputs {
                file: &entry.file,
                sizes: (text[0].len() as u64, text[1].len() as u64),
            },
        ) else {
            bail!("search requires text files")
        };
        let stored = StoredDiff {
            entry,
            sources: sides,
            stats,
            hidden,
        };
        self.store.put(&cache_key, &stored)?;
        Ok(stored)
    }
}

fn sides<T>(pair: &Pairing<T>) -> Vec<(bool, &T)> {
    match pair {
        Pairing::Both { lhs, rhs } => vec![(false, lhs), (true, rhs)],
        Pairing::LeftOnly { lhs } => vec![(false, lhs)],
        Pairing::RightOnly { rhs } => vec![(true, rhs)],
    }
}
fn key(file: &Pairing<FileRef>) -> String {
    serde_json::to_string(file).expect("file refs serialize")
}
fn oid(hex: &str) -> anyhow::Result<Oid> {
    Oid::from_hex(hex.as_bytes()).map_err(|error| anyhow!("{error}"))
}
fn validate(scope: &Scope) -> anyhow::Result<()> {
    ensure!(
        scope.repo.is_absolute(),
        "scope repository must be absolute"
    );
    let repo = gix::open(&scope.repo)?;
    for revision in [&scope.base, &scope.head] {
        let pinned = oid(revision)?;
        ensure!(
            pinned.to_string() == *revision,
            "scope revisions must be full commit IDs"
        );
        repo.find_commit(pinned)?;
    }
    Ok(())
}
fn commit(repo: &Repository, revision: &str) -> anyhow::Result<Oid> {
    Ok(repo
        .rev_parse_single(revision)
        .with_context(|| format!("invalid hit/comparison revision {revision:?}"))?
        .object()?
        .peel_to_commit()?
        .id()
        .detach())
}
fn side_mut<T>(pair: &mut Pairing<T>, right: bool) -> Option<&mut T> {
    match (pair, right) {
        (Pairing::Both { lhs, .. } | Pairing::LeftOnly { lhs }, false) => Some(lhs),
        (Pairing::Both { rhs, .. } | Pairing::RightOnly { rhs }, true) => Some(rhs),
        _ => None,
    }
}
fn attach(regions: &mut [Region], spans: &[Span]) {
    for region in regions {
        match &mut region.node {
            Node::Leaf {
                search_highlights, ..
            } => {
                *search_highlights = spans
                    .iter()
                    .copied()
                    .filter(|s| region.range.lines().contains(&s.line))
                    .collect();
            }
            Node::Fold { children, .. } => attach(children, spans),
        }
    }
}
fn collect(regions: &[Region], spans: &mut Vec<Span>) {
    for region in regions {
        match &region.node {
            Node::Leaf {
                search_highlights, ..
            } => spans.extend(search_highlights),
            Node::Fold { children, .. } => collect(children, spans),
        }
    }
}
fn add_highlights(regions: &mut [Region], new: Vec<Span>) {
    let mut spans = new;
    collect(regions, &mut spans);
    spans.sort_by_key(|s| (s.line, s.start_column, s.end_column));
    spans.dedup();
    attach(regions, &spans);
}
