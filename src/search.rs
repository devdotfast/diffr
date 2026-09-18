//! Search owns pinned-blob indexing and hit hydration. Ordinary diff fast paths
//! are unchanged; plugin inputs use the shared source/region schema.
use crate::{
    config::Config,
    git,
    pairing::Pairing,
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
enum ViewKind {
    Combined,
    Lhs,
    Rhs,
    Unchanged,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchResult {
    kind: ViewKind,
    scope: Scope,
    file: Pairing<FileRef>,
    sources: Pairing<Source>,
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

// The host keeps one active index. Only a storage backend/location change replaces
// it; scopes and analysis settings do not participate in its lifetime.
type ConfiguredIndex = (DiffKey, Arc<Index<dyn Store>>);
static INDEX: LazyLock<Mutex<Option<ConfiguredIndex>>> = LazyLock::new(|| Mutex::new(None));

/// Create comparison-local analysis state using the host's shared computed index.
/// Storage remains entirely outside the serialized client API.
pub fn configured_session(scope: Scope, options: Options) -> anyhow::Result<Session<dyn Store>> {
    let mut config = Config::load(None)?;
    if let Some(mut plugins) = options.plugins {
        plugins.resolve(&scope.repo)?;
        config.plugins = plugins;
    }
    let index = index_for_config(&scope.repo, &config)?;
    Session::with_config(scope, index, config)
}

fn index_for_config(repository: &Path, config: &Config) -> anyhow::Result<Arc<Index<dyn Store>>> {
    let repository = repository.canonicalize()?;
    let directory = match config.storage.backend {
        crate::storage::StoreBackend::Memory => None,
        _ => Some(repository.join(&config.storage.path)),
    };
    let key = DiffKey::new(&(config.storage.backend, directory))?;
    let mut current = INDEX
        .lock()
        .map_err(|_| anyhow!("search index lock poisoned"))?;
    if let Some((identity, index)) = &*current {
        if identity == &key {
            return Ok(index.clone());
        }
    }
    let index = Arc::new(Index::new(config.storage.open(&repository)?));
    *current = Some((key, index.clone()));
    Ok(index)
}

impl<S: Store + ?Sized> Session<S> {
    /// Create a comparison using an existing index and default analysis settings.
    pub fn new(scope: Scope, index: Arc<Index<S>>) -> anyhow::Result<Self> {
        Self::with_config(scope, index, Config::default())
    }

    fn with_config(scope: Scope, index: Arc<Index<S>>, config: Config) -> anyhow::Result<Self> {
        validate(&scope)?;
        let pipeline = Pipeline::from_config(&config.plugins, &scope.repo)?;
        let queries = pipeline.queries()?;
        let resolved: BTreeMap<_, Vec<_>> = crate::plugin::queries::assemble(&queries)?
            .into_iter()
            .map(|(language, sources)| {
                (
                    language,
                    sources
                        .into_iter()
                        .map(|source| (source.name, source.text))
                        .collect(),
                )
            })
            .collect();
        let analysis = DiffKey::new(&(
            serde_json::to_value(&config.diff)?,
            serde_json::to_value(&config.plugins)?,
            resolved,
        ))?
        .as_str()
        .to_owned();
        let params = Arc::new(config.compile_queries(queries)?);
        let session = git::DiffSession::open(
            &scope.repo,
            git::Comparison {
                before: git::Operand::revision(&scope.base_worktree.commit_id),
                after: git::Operand::revision(&scope.head_worktree.commit_id),
            },
            params.clone(),
            &git::FileParams::default(),
            &pipeline,
        )
        .map_err(|e| anyhow!("{e}"))?;
        Ok(Self {
            scope,
            index,
            params,
            pipeline,
            manifest: session
                .file_manifest()
                .iter()
                .map(|f| f.manifest_entry())
                .collect(),
            analysis,
        })
    }
    /// Hydrate line hits against pinned Git blobs. Cached structural trees never
    /// contain query-specific spans or mutable presentation state.
    pub fn hydrate(&mut self, hits: Vec<Hit>) -> anyhow::Result<Vec<SearchResult>> {
        validate(&self.scope)?;
        let scope = self.scope.clone();
        let mut selected: BTreeMap<String, SearchResult> = BTreeMap::new();
        for hit in hits {
            ensure!(hit.file.is_absolute(), "hit paths must be absolute");
            let path = hit.file.canonicalize().context("hit path does not exist")?;
            let mut located = None;
            for (right, worktree) in [(false, &scope.base_worktree), (true, &scope.head_worktree)] {
                if let Ok(relative) = path.strip_prefix(worktree.path.canonicalize()?) {
                    located = Some((
                        right,
                        relative.to_str().context("non-UTF8 hit path")?.to_owned(),
                    ));
                }
            }
            let (right, path) = located.context("hit path is outside scoped worktrees")?;
            let indexed = self.index.resolve(self, right, &path)?;
            let key = key(&indexed.entry.file);
            let result = selected.entry(key).or_insert_with(|| {
                let kind = if !sides(&indexed.sources)
                    .iter()
                    .any(|(_, source)| changed(&source.regions))
                {
                    ViewKind::Unchanged
                } else {
                    match indexed.sources {
                        Pairing::Both { .. } => ViewKind::Combined,
                        Pairing::LeftOnly { .. } => ViewKind::Lhs,
                        Pairing::RightOnly { .. } => ViewKind::Rhs,
                    }
                };
                SearchResult {
                    kind,
                    scope: scope.clone(),
                    file: indexed.entry.file.clone(),
                    sources: indexed.sources.clone().map(|mut source| {
                        source.regions.clear();
                        source
                    }),
                }
            });
            let source = sides(&indexed.sources)
                .into_iter()
                .find(|(side, _)| *side == right)
                .unwrap()
                .1;
            let output = sides_mut(&mut result.sources)
                .into_iter()
                .find(|(side, _)| *side == right)
                .unwrap()
                .1;
            for line in hit.lines {
                ensure!(line.line > 0, "hit line numbers are 1-based");
                let text = source
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
                let span = Span {
                    line: line.line - 1,
                    start_column: 0,
                    end_column: u32::try_from(text.len())?,
                };
                let mut region =
                    candidates(&source.regions, span.line).context("hit has no source region")?;
                attach(std::slice::from_mut(&mut region), &[span]);
                // Initial visibility shows evidence only; postprocessing supplies context.
                evidence_visibility(std::slice::from_mut(&mut region));
                output.regions.push(region);
            }
        }
        Ok(selected.into_values().collect())
    }

    /// Coalesce selected candidates, restore complete indexed trees, and run the
    /// same plugin pipeline used for ordinary diffs with query spans attached.
    pub fn postprocess(
        &mut self,
        selected: Vec<SearchResult>,
    ) -> anyhow::Result<Vec<SearchResult>> {
        validate(&self.scope)?;
        let scope = self.scope.clone();
        let mut groups: BTreeMap<String, SearchResult> = BTreeMap::new();
        for result in selected {
            ensure!(
                result.scope == scope,
                "selected result belongs to another scope"
            );
            ensure!(
                sides(&result.file)
                    .iter()
                    .map(|(side, _)| side)
                    .eq(sides(&result.sources).iter().map(|(side, _)| side)),
                "file and source sides must agree"
            );
            ensure!(
                match result.kind {
                    ViewKind::Combined => matches!(result.sources, Pairing::Both { .. }),
                    ViewKind::Lhs => matches!(result.sources, Pairing::LeftOnly { .. }),
                    ViewKind::Rhs => matches!(result.sources, Pairing::RightOnly { .. }),
                    ViewKind::Unchanged => true,
                },
                "result kind and sides must agree"
            );
            let group_key = format!(
                "{}:{}",
                serde_json::to_string(&result.kind)?,
                key(&result.file)
            );
            if let Some(group) = groups.get_mut(&group_key) {
                for (right, source) in sides_mut(&mut group.sources) {
                    if let Some((_, incoming)) = sides(&result.sources)
                        .into_iter()
                        .find(|(side, _)| *side == right)
                    {
                        source.regions.extend(incoming.regions.clone());
                    }
                }
            } else {
                groups.insert(group_key, result);
            }
        }
        let mut results = Vec::new();
        for (_, mut result) in groups {
            let (side, file) = sides(&result.file)[0];
            let indexed = self.index.resolve(self, side, &file.path)?;
            for (right, file) in sides(&result.file) {
                ensure!(
                    sides(&indexed.entry.file)
                        .iter()
                        .any(|(s, f)| *s == right && *f == file),
                    "selected file identity differs from index"
                );
            }
            for (right, source) in sides_mut(&mut result.sources) {
                let original = sides(&indexed.sources)
                    .into_iter()
                    .find(|(s, _)| *s == right)
                    .context("selected side absent from index")?
                    .1;
                ensure!(
                    source.text == original.text,
                    "selected source text differs from index"
                );
                let mut spans = Vec::new();
                collect(&source.regions, &mut spans);
                spans.sort_by_key(|span| (span.line, span.start_column, span.end_column));
                spans.dedup();
                for span in &spans {
                    let line = original
                        .text
                        .split_terminator('\n')
                        .nth(span.line as usize)
                        .context("highlight line out of bounds")?;
                    ensure!(
                        span.start_column <= span.end_column
                            && span.end_column as usize <= line.len()
                            && line.is_char_boundary(span.start_column as usize)
                            && line.is_char_boundary(span.end_column as usize),
                        "invalid highlight coordinates"
                    );
                }
                let mut merged: Vec<Span> = Vec::new();
                for span in spans {
                    if let Some(last) = merged.last_mut().filter(|last| {
                        last.line == span.line && span.start_column <= last.end_column
                    }) {
                        last.end_column = last.end_column.max(span.end_column);
                    } else {
                        merged.push(span);
                    }
                }
                *source = original.clone();
                attach(&mut source.regions, &merged);
            }
            let entry = FileChange {
                file: result.file.clone(),
                ..indexed.entry.clone()
            };
            self.pipeline.run(&entry, &mut result.sources)?;
            results.push(result);
        }
        Ok(results)
    }
}

impl<S: Store + ?Sized> Index<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }

    fn resolve(&self, session: &Session<S>, right: bool, path: &str) -> anyhow::Result<StoredDiff> {
        let scope = &session.scope;
        let entry = if let Some(entry) = session.manifest.iter().find(|entry| {
            sides(&entry.file)
                .iter()
                .any(|(side, file)| *side == right && file.path == path)
        }) {
            entry.clone()
        } else {
            let repo = Repository::open(&scope.repo)?;
            let file = |worktree: &Worktree| -> anyhow::Result<FileRef> {
                let tree = repo
                    .find_commit(Oid::from_str(&worktree.commit_id)?)?
                    .tree()?;
                let entry = tree
                    .get_path(Path::new(path))
                    .context("hit does not resolve to an indexed blob")?;
                ensure!(
                    entry.kind() == Some(git2::ObjectType::Blob)
                        && entry.filemode() & 0o170000 == 0o100000,
                    "search requires regular text files"
                );
                Ok(FileRef {
                    path: path.to_owned(),
                    oid: entry.id().to_string(),
                    mode: format!("{:o}", entry.filemode()),
                })
            };
            let lhs = file(&scope.base_worktree)?;
            let rhs = file(&scope.head_worktree)?;
            ensure!(
                lhs.oid == rhs.oid,
                "changed file missing from comparison manifest"
            );
            let mut entry = FileChange {
                file: Pairing::Both { lhs, rhs },
                status: FileStatus::Unchanged,
                tags: crate::tags::from_path(path)
                    .into_iter()
                    .map(str::to_owned)
                    .collect(),
            };
            entry.tags = session.pipeline.classify(&entry)?;
            entry
        };
        let cache_key = DiffKey::new(&(&session.analysis, path, &entry))?;
        if let Some(diff) = self.store.get(&cache_key)? {
            return Ok(diff);
        }
        let repo = Repository::open(&scope.repo)?;
        let read = |file: &FileRef| -> anyhow::Result<String> {
            ensure!(
                file.mode == "100644" || file.mode == "100755",
                "search requires regular text files"
            );
            let blob = repo.find_blob(Oid::from_str(&file.oid)?)?;
            ensure!(!blob.is_binary(), "search requires text files");
            Ok(std::str::from_utf8(blob.content())?.to_owned())
        };
        let mut text = [String::new(), String::new()];
        for (right, file) in sides(&entry.file) {
            text[usize::from(right)] = read(file)?;
        }
        let options = crate::options::DiffOptions {
            generated: entry.tags.iter().any(|tag| tag == "generated"),
            ..session.params.diff.options(false)
        };
        let mut diff = DiffResult::from_sources_with_options(
            path,
            &text[0],
            &text[1],
            &session.params,
            &options,
        )?;
        // Parse an identical blob once for search context, independently of diffing.
        if text[0] == text[1] && !options.generated && text[0].len() <= options.byte_limit {
            if let Some(language) =
                crate::parse::guess_language::guess(Path::new(path), &text[0], &[])
            {
                let config = session.params.language(language);
                let tree = crate::parse::tree_sitter_parser::to_tree(&text[0], config.parser);
                let arena = typed_arena::Arena::new();
                let (nodes, _) = crate::parse::tree_sitter_parser::to_syntax(
                    &tree, &text[0], &arena, config, false,
                )
                .map_err(|e| anyhow!("fold query conflict: {e:?}"))?;
                crate::parse::folds::unmatched(&nodes, &mut diff.lhs_folds);
                let count = diff.lhs_folds.len();
                for (i, fold) in diff.lhs_folds.iter_mut().enumerate() {
                    let lhs = std::num::NonZeroU32::new(i as u32 + 1).unwrap();
                    let rhs = std::num::NonZeroU32::new((count + i) as u32 + 1).unwrap();
                    fold.syntax_id = lhs;
                    fold.match_kind = crate::parse::folds::FoldMatch::Matched { opposite: rhs };
                    diff.rhs_folds.push(crate::parse::folds::Fold {
                        tags: fold.tags.clone(),
                        range: fold.range,
                        syntax_id: rhs,
                        match_kind: crate::parse::folds::FoldMatch::Matched { opposite: lhs },
                        placeholder: String::new(),
                    });
                }
            }
        }
        let syntax = |source: &str| {
            crate::parse::guess_language::guess(Path::new(path), source, &[])
                .map(|language| {
                    protocol::project::syntax_spans(
                        source,
                        session.params.language(language).parser,
                    )
                })
                .unwrap_or_default()
        };
        let protocol::Diff::Text { sides, .. } = protocol::project::diff(
            &diff,
            protocol::project::Inputs {
                file: &entry.file,
                sizes: (text[0].len() as u64, text[1].len() as u64),
                syntax: (syntax(&text[0]), syntax(&text[1])),
            },
        ) else {
            bail!("search requires text files")
        };
        let stored = StoredDiff {
            entry,
            sources: sides,
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
fn sides_mut<T>(pair: &mut Pairing<T>) -> Vec<(bool, &mut T)> {
    match pair {
        Pairing::Both { lhs, rhs } => vec![(false, lhs), (true, rhs)],
        Pairing::LeftOnly { lhs } => vec![(false, lhs)],
        Pairing::RightOnly { rhs } => vec![(true, rhs)],
    }
}
fn key(file: &Pairing<FileRef>) -> String {
    serde_json::to_string(file).expect("file refs serialize")
}
fn validate(scope: &Scope) -> anyhow::Result<()> {
    let repo = Repository::open(&scope.repo)?;
    for worktree in [&scope.base_worktree, &scope.head_worktree] {
        ensure!(
            worktree.path.is_absolute(),
            "worktree paths must be absolute"
        );
        let pinned = Oid::from_str(&worktree.commit_id)?;
        repo.find_commit(pinned)?;
        let checkout = Repository::open(&worktree.path)?;
        ensure!(
            checkout.head()?.target() == Some(pinned),
            "worktree HEAD does not match its commitId"
        );
        ensure!(
            checkout.commondir().canonicalize()? == repo.commondir().canonicalize()?,
            "worktree belongs to another repository"
        );
    }
    ensure!(
        scope.base_worktree.path.canonicalize()? != scope.head_worktree.path.canonicalize()?,
        "worktrees must be distinct"
    );
    Ok(())
}
fn changed(regions: &[Region]) -> bool {
    regions.iter().any(|r| match &r.node {
        Node::Leaf { changed, .. } => !changed.is_empty(),
        Node::Fold { children } => changed(children),
    })
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
            Node::Fold { children } => attach(children, spans),
        }
    }
}
fn collect(regions: &[Region], spans: &mut Vec<Span>) {
    for region in regions {
        match &region.node {
            Node::Leaf {
                search_highlights, ..
            } => spans.extend(search_highlights),
            Node::Fold { children } => collect(children, spans),
        }
    }
}
fn candidates(regions: &[Region], line: u32) -> Option<Region> {
    let region = regions.iter().find(|r| r.range.lines().contains(&line))?;
    // Query-defined top-level scopes keep a candidate's structural context.
    Some(region.clone())
}

fn evidence_visibility(regions: &mut [Region]) -> bool {
    let mut any = false;
    for region in regions {
        let highlighted = match &mut region.node {
            Node::Leaf {
                search_highlights, ..
            } => !search_highlights.is_empty(),
            Node::Fold { children } => evidence_visibility(children),
        };
        region.visibility.collapsed = !highlighted;
        any |= highlighted;
    }
    any
}

#[cfg(test)]
mod storage_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct ReadOnlyStore {
        store: Arc<dyn Store>,
        hits: AtomicUsize,
    }
    impl Store for ReadOnlyStore {
        fn get(&self, key: &DiffKey) -> anyhow::Result<Option<StoredDiff>> {
            let stored = self.store.get(key)?;
            if let Some(diff) = &stored {
                self.hits.fetch_add(1, Ordering::SeqCst);
                let text = serde_json::to_string(diff)?;
                assert!(!text.contains("search_highlights"));
                assert!(!text.contains("\"collapsed\":true"));
            }
            Ok(stored)
        }
        fn put(&self, _: &DiffKey, _: &StoredDiff) -> anyhow::Result<()> {
            bail!("unexpected recomputation on a warm store")
        }
        fn remove(&self, key: &DiffKey) -> anyhow::Result<()> {
            self.store.remove(key)
        }
    }
    fn fixture() -> (tempfile::TempDir, Scope, Vec<Hit>) {
        let temporary = tempfile::tempdir().unwrap();
        let repo = temporary.path().join("repo");
        std::fs::create_dir(&repo).unwrap();
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=Fixture",
                    "-c",
                    "user.email=fixture@example.invalid",
                    "-c",
                    "commit.gpgSign=false",
                    "-c",
                    "core.hooksPath=/dev/null",
                ])
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8(output.stdout).unwrap().trim().to_owned()
        };
        git(&["init", "--quiet"]);
        std::fs::write(
            repo.join("retry.js"),
            "function retry() {\n  return 1; // token\n}\n",
        )
        .unwrap();
        std::fs::write(
            repo.join("same.js"),
            "function same() {\n  return 'token';\n}\n",
        )
        .unwrap();
        git(&["add", "."]);
        git(&["commit", "--quiet", "-m", "base"]);
        let base = git(&["rev-parse", "HEAD"]);
        std::fs::write(
            repo.join("retry.js"),
            "function retry() {\n  return 2; // token\n}\n",
        )
        .unwrap();
        git(&["commit", "--quiet", "-am", "head"]);
        let head = git(&["rev-parse", "HEAD"]);
        let base_path = temporary.path().join("base");
        let head_path = temporary.path().join("head");
        git(&[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            base_path.to_str().unwrap(),
            &base,
        ]);
        git(&[
            "worktree",
            "add",
            "--quiet",
            "--detach",
            head_path.to_str().unwrap(),
            &head,
        ]);
        let scope = Scope {
            repo,
            base_worktree: Worktree {
                path: base_path,
                commit_id: base,
            },
            head_worktree: Worktree {
                path: head_path.clone(),
                commit_id: head,
            },
        };
        let hits = vec![
            Hit {
                file: head_path.join("retry.js"),
                lines: vec![HitLine {
                    line: 2,
                    text: "  return 2; // token".into(),
                }],
            },
            Hit {
                file: head_path.join("same.js"),
                lines: vec![HitLine {
                    line: 2,
                    text: "  return 'token';".into(),
                }],
            },
        ];
        (temporary, scope, hits)
    }
    #[test]
    fn separate_comparisons_reuse_the_same_index_and_clean_computed_trees() {
        let (_temporary, scope, hits) = fixture();
        let memory = Arc::new(crate::storage::MemoryStore::default());
        let mut warm = Session::new(scope.clone(), Arc::new(Index::new(memory.clone()))).unwrap();
        warm.hydrate(hits.clone()).unwrap();

        // From here on, any cache miss fails instead of recomputing a diff.
        let reader = Arc::new(ReadOnlyStore {
            store: memory,
            hits: AtomicUsize::new(0),
        });
        let index = Arc::new(Index::new(reader.clone()));
        let mut first = Session::new(scope.clone(), index.clone()).unwrap();
        let first_results = first.hydrate(hits).unwrap();
        first.postprocess(first_results).unwrap();
        drop(first);

        let reversed = Scope {
            repo: scope.repo.clone(),
            base_worktree: scope.head_worktree.clone(),
            head_worktree: scope.base_worktree.clone(),
        };
        let mut second = Session::new(reversed, index).unwrap();
        let results = second
            .hydrate(vec![Hit {
                file: scope.base_worktree.path.join("same.js"),
                lines: vec![HitLine {
                    line: 1,
                    text: "function same() {".into(),
                }],
            }])
            .unwrap();
        second.postprocess(results).unwrap();
        assert_eq!(reader.hits.load(Ordering::SeqCst), 6);
    }

    #[test]
    fn config_selects_storage_and_reopened_stores_do_not_recompute() {
        let (temporary, scope, hits) = fixture();
        let repository = &scope.repo;
        let mut expected = None;
        for backend in ["memory", "file", "sqlite"] {
            let config_path = temporary.path().join("config.toml");
            std::fs::write(
                &config_path,
                format!("[storage]\nbackend = '{backend}'\npath = '.cache/diffr'\n"),
            )
            .unwrap();
            let config = Config::load(Some(&config_path)).unwrap();
            let index = index_for_config(repository, &config).unwrap();
            let mut changed_analysis = config.clone();
            changed_analysis.plugins = Config::from_toml("[plugins]\norder = []").unwrap().plugins;
            assert!(Arc::ptr_eq(
                &index,
                &index_for_config(repository, &changed_analysis).unwrap()
            ));
            let mut session = Session::with_config(scope.clone(), index, config.clone()).unwrap();
            let hydrated = session.hydrate(hits.clone()).unwrap();
            let result = serde_json::to_value(session.postprocess(hydrated).unwrap()).unwrap();
            if let Some(expected) = &expected {
                assert_eq!(&result, expected);
            } else {
                expected = Some(result.clone());
            }
            if backend == "memory" {
                continue;
            }
            let reader = Arc::new(ReadOnlyStore {
                store: config.storage.open(repository).unwrap(),
                hits: AtomicUsize::new(0),
            });
            let mut reopened =
                Session::with_config(scope.clone(), Arc::new(Index::new(reader.clone())), config)
                    .unwrap();
            let hydrated = reopened.hydrate(hits.clone()).unwrap();
            assert_eq!(
                serde_json::to_value(reopened.postprocess(hydrated).unwrap()).unwrap(),
                result
            );
            assert!(reader.hits.load(Ordering::SeqCst) >= 4);
            // Different hit locations reuse clean trees rather than the old query's spans.
            let alternate = vec![Hit {
                file: scope.head_worktree.path.join("same.js"),
                lines: vec![HitLine {
                    line: 1,
                    text: "function same() {".into(),
                }],
            }];
            let mut fresh = Session::new(
                scope.clone(),
                Arc::new(Index::new(Arc::new(crate::storage::MemoryStore::default()))),
            )
            .unwrap();
            assert_eq!(
                serde_json::to_value(reopened.hydrate(alternate.clone()).unwrap()).unwrap(),
                serde_json::to_value(fresh.hydrate(alternate).unwrap()).unwrap()
            );
            // Analysis changes must miss, even when the blob identities are unchanged.
            let other_config = Config::from_toml("[plugins]\norder = []").unwrap();
            let mut other =
                Session::with_config(scope.clone(), Arc::new(Index::new(reader)), other_config)
                    .unwrap();
            assert!(other
                .hydrate(hits.clone())
                .unwrap_err()
                .to_string()
                .contains("unexpected recomputation"));
        }
        assert!(repository.join(".cache/diffr/diffs.sqlite").exists());
        assert!(!temporary.path().join(".cache").exists());
    }
}
