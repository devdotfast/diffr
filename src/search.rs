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
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
};

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Worktree {
    commit_id: String,
    path: PathBuf,
}
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Scope {
    repo: PathBuf,
    base_worktree: Worktree,
    head_worktree: Worktree,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Hit {
    file: PathBuf,
    lines: Vec<HitLine>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HitLine {
    line: u32,
    text: String,
}
#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ViewKind {
    Combined,
    Lhs,
    Rhs,
    Unchanged,
}
#[derive(Clone, Serialize, Deserialize)]
struct SearchResult {
    kind: ViewKind,
    scope: Scope,
    file: Pairing<FileRef>,
    sources: Pairing<Source>,
}
struct IndexedFile {
    entry: FileChange,
    sources: Pairing<Source>,
}
struct Index {
    params: Arc<crate::config::Params>,
    pipeline: Pipeline,
    manifest: Vec<FileChange>,
    files: HashMap<String, IndexedFile>,
}
static INDEXES: LazyLock<Mutex<HashMap<Scope, Index>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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
impl Index {
    fn new(scope: &Scope, config: Config) -> anyhow::Result<Self> {
        let pipeline = Pipeline::from_config(&config.plugins, &scope.repo)?;
        let params = Arc::new(config.compile_with(&pipeline)?);
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
            params,
            pipeline,
            manifest: session
                .file_manifest()
                .iter()
                .map(|f| f.manifest_entry())
                .collect(),
            files: HashMap::new(),
        })
    }
    fn resolve(&mut self, scope: &Scope, right: bool, path: &str) -> anyhow::Result<String> {
        let entry = if let Some(entry) = self.manifest.iter().find(|entry| {
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
            entry.tags = self.pipeline.classify(&entry)?;
            entry
        };
        let key = key(&entry.file);
        if !self.files.contains_key(&key) {
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
                ..self.params.diff.options(false)
            };
            let mut diff = DiffResult::from_sources_with_options(
                path,
                &text[0],
                &text[1],
                &self.params,
                &options,
            )?;
            // Parse an identical blob once for search context, independently of diffing.
            if text[0] == text[1] && !options.generated && text[0].len() <= options.byte_limit {
                if let Some(language) =
                    crate::parse::guess_language::guess(Path::new(path), &text[0], &[])
                {
                    let config = self.params.language(language);
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
                            self.params.language(language).parser,
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
            self.files.insert(
                key.clone(),
                IndexedFile {
                    entry,
                    sources: sides,
                },
            );
        }
        Ok(key)
    }
}
fn with_index<T>(
    scope: &Scope,
    f: impl FnOnce(&mut Index) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    validate(scope)?;
    let mut indexes = INDEXES
        .lock()
        .map_err(|_| anyhow!("search index lock poisoned"))?;
    if !indexes.contains_key(scope) {
        if indexes.len() >= 8 {
            indexes.clear();
        }
        indexes.insert(scope.clone(), Index::new(scope, Config::default())?);
    }
    f(indexes.get_mut(scope).unwrap())
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

/// Hydrate line hits against pinned Git blobs. Cached structural trees never
/// contain query-specific spans or mutable presentation state.
pub fn hydrate(scope: Value, hits: Value) -> anyhow::Result<Value> {
    let scope: Scope = serde_json::from_value(scope)?;
    let hits: Vec<Hit> = serde_json::from_value(hits)?;
    with_index(&scope, |index| {
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
            let key = index.resolve(&scope, right, &path)?;
            let indexed = &index.files[&key];
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
        Ok(serde_json::to_value(
            selected.into_values().collect::<Vec<_>>(),
        )?)
    })
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

/// Coalesce selected candidates, restore complete indexed trees, and run the
/// same plugin pipeline used for ordinary diffs with query spans attached.
pub fn postprocess(scope: Value, selected: Value, options: Option<Value>) -> anyhow::Result<Value> {
    let scope: Scope = serde_json::from_value(scope)?;
    let selected: Vec<SearchResult> = serde_json::from_value(selected)?;
    let process = |index: &mut Index| {
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
            let indexed_key = index.resolve(&scope, side, &file.path)?;
            let indexed = &index.files[&indexed_key];
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
            index.pipeline.run(&entry, &mut result.sources)?;
            results.push(result);
        }
        Ok(serde_json::to_value(results)?)
    };
    #[derive(Deserialize, Default)]
    #[serde(deny_unknown_fields)]
    struct Options {
        plugins: Option<crate::plugin::config::PluginsConfig>,
    }
    let options: Options = options
        .map(serde_json::from_value)
        .transpose()?
        .unwrap_or_default();
    if let Some(mut plugins) = options.plugins {
        validate(&scope)?;
        plugins.resolve(&scope.repo)?;
        let mut index = Index::new(
            &scope,
            Config {
                plugins,
                ..Config::default()
            },
        )?;
        process(&mut index)
    } else {
        with_index(&scope, process)
    }
}
