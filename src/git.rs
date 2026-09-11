//! Git comparison selection and lazy source loading used by the CLI and its stdout stream.
use crate::config::Params;
use crate::parse::guess_language::{guess, language_name};
use crate::protocol;
use crate::summary::DiffResult;
use git2::{AttrCheckFlags, AttrValue, Delta, Diff, DiffFindOptions, DiffOptions, Oid, Repository};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Operand {
    Revision { r#ref: String },
    Index,
    WorkingTree,
    EmptyTree,
}

impl Operand {
    pub(crate) fn revision(reference: impl Into<String>) -> Self {
        Self::Revision {
            r#ref: reference.into(),
        }
    }

    fn resolve(&self, repo: &Repository) -> Result<Self> {
        match self {
            Self::Revision { r#ref } => {
                let object = repo.revparse_single(r#ref)?;
                // Retain commit identity where possible, while accepting tree objects too.
                let id = match object.peel_to_commit() {
                    Ok(commit) => commit.id(),
                    Err(_) => object.peel_to_tree()?.id(),
                };
                Ok(Self::revision(id.to_string()))
            }
            _ => Ok(self.clone()),
        }
    }

    fn tree<'a>(&self, repo: &'a Repository) -> Result<Option<git2::Tree<'a>>> {
        match self {
            Self::Revision { r#ref } => Ok(Some(repo.revparse_single(r#ref)?.peel_to_tree()?)),
            Self::EmptyTree => Ok(None),
            _ => Err("expected a revision or empty tree".into()),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Comparison {
    pub(crate) before: Operand,
    pub(crate) after: Operand,
}

impl Comparison {
    pub(crate) fn resolve(&self, repo: &Repository) -> Result<Self> {
        Ok(Self {
            before: self.before.resolve(repo)?,
            after: self.after.resolve(repo)?,
        })
    }

    pub(crate) fn reverse(&mut self) {
        std::mem::swap(&mut self.before, &mut self.after);
    }

    pub(crate) fn diff<'a>(&self, repo: &'a Repository, files: &FileParams) -> Result<Diff<'a>> {
        use Operand::*;
        let (before, after, reverse) = match (&self.before, &self.after) {
            (WorkingTree, _) | (Index, Revision { .. } | EmptyTree) => {
                (&self.after, &self.before, true)
            }
            _ => (&self.before, &self.after, false),
        };
        let mut options = DiffOptions::new();
        options.include_typechange(true).reverse(reverse);
        for path in &files.paths {
            // libgit2 supports directory prefixes and wildcards, but not Git's magic pathspec DSL.
            if path.starts_with(':') {
                return Err(
                    "magic pathspecs are not supported yet; use paths or wildcard patterns".into(),
                );
            }
            options.pathspec(path);
        }
        let mut diff = match (before, after) {
            (Revision { .. } | EmptyTree, Revision { .. } | EmptyTree) => repo.diff_tree_to_tree(
                before.tree(repo)?.as_ref(),
                after.tree(repo)?.as_ref(),
                Some(&mut options),
            )?,
            (Revision { .. } | EmptyTree, Index) => {
                repo.diff_tree_to_index(before.tree(repo)?.as_ref(), None, Some(&mut options))?
            }
            (Index, WorkingTree) => repo.diff_index_to_workdir(None, Some(&mut options))?,
            (Revision { .. } | EmptyTree, WorkingTree) => repo
                .diff_tree_to_workdir_with_index(before.tree(repo)?.as_ref(), Some(&mut options))?,
            _ => {
                return Err(
                    "compare two revisions, a revision and index/worktree, or index and worktree"
                        .into(),
                )
            }
        };
        if files.renames {
            diff.find_similar(Some(DiffFindOptions::new().renames(true)))?;
        }
        Ok(diff)
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    TypeChanged,
    Conflicted,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct FileChange {
    pub(crate) old_path: Option<String>,
    pub(crate) new_path: Option<String>,
    pub(crate) status: FileStatus,
    pub(crate) class: Option<String>,
    /// Git's delta sides. Only the v2 wire carries them.
    #[serde(skip)]
    pub(crate) sides: protocol::Pairing<protocol::FileRef>,
    #[serde(skip)]
    pub(crate) language: Option<String>,
}

impl FileChange {
    pub(crate) fn path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .expect("changed file has a path")
    }

    /// A standalone comparison of two paths, outside any repository.
    pub(crate) fn standalone(before: &str, after: &str) -> Self {
        let file_ref = |path: &str| protocol::FileRef {
            path: path.to_owned(),
            oid: String::new(),
            mode: String::new(),
        };
        let old_path = (before != "/dev/null").then(|| before.to_owned());
        let new_path = (after != "/dev/null").then(|| after.to_owned());
        let (status, sides) = match (&old_path, &new_path) {
            (Some(old), Some(new)) => (
                FileStatus::Modified,
                protocol::Pairing::Both {
                    lhs: file_ref(old),
                    rhs: file_ref(new),
                },
            ),
            (Some(old), None) => (
                FileStatus::Deleted,
                protocol::Pairing::LeftOnly { lhs: file_ref(old) },
            ),
            (None, Some(new)) => (
                FileStatus::Added,
                protocol::Pairing::RightOnly { rhs: file_ref(new) },
            ),
            (None, None) => panic!("a standalone comparison needs at least one path"),
        };
        let path = new_path.as_deref().or(old_path.as_deref()).expect("a path");
        let language = language_of(path);
        let class = crate::category::from_path(path).map(str::to_owned);
        Self {
            old_path,
            new_path,
            status,
            class,
            sides,
            language,
        }
    }

    pub(crate) fn manifest_entry(&self) -> protocol::FileChange {
        protocol::FileChange {
            file: self.sides.clone(),
            status: match self.status {
                FileStatus::Added => protocol::FileStatus::Added,
                FileStatus::Deleted => protocol::FileStatus::Deleted,
                FileStatus::Modified => protocol::FileStatus::Modified,
                FileStatus::Renamed => protocol::FileStatus::Renamed,
                FileStatus::TypeChanged => protocol::FileStatus::TypeChanged,
                // Both sides exist; the file record carries the unmerged error.
                FileStatus::Conflicted => protocol::FileStatus::Modified,
            },
            category: self.class.clone(),
            language: self.language.clone(),
            visibility: protocol::Visibility::default(),
        }
    }
}

/// `diffr-classify` wins; then `linguist-generated`; then the built-in
/// path rules.
fn category(repo: &Repository, path: &str) -> Result<Option<String>> {
    let attr = |name: &str| {
        repo.get_attr(Path::new(path), name, AttrCheckFlags::FILE_THEN_INDEX)
            .map(AttrValue::from_string)
    };
    if let AttrValue::String(value) = attr("diffr-classify")? {
        return Ok(Some(value.to_owned()));
    }
    if let AttrValue::True = attr("linguist-generated")? {
        return Ok(Some(crate::category::GENERATED.to_owned()));
    }
    Ok(crate::category::from_path(path).map(str::to_owned))
}

fn language_of(path: &str) -> Option<String> {
    guess(Path::new(path), "", &[]).map(|language| language_name(language).to_owned())
}

/// Why one file could not be diffed. `code` is stable on the wire.
#[derive(Debug)]
pub(crate) struct FileProblem {
    pub(crate) code: &'static str,
    pub(crate) message: String,
}

impl FileProblem {
    fn new(code: &'static str, message: impl fmt::Display) -> Self {
        Self {
            code,
            message: message.to_string(),
        }
    }
}

impl fmt::Display for FileProblem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FileProblem {}

impl From<&Operand> for protocol::Snapshot {
    fn from(operand: &Operand) -> Self {
        match operand {
            Operand::Revision { r#ref } => Self::Revision { rev: r#ref.clone() },
            Operand::Index => Self::Index,
            Operand::WorkingTree => Self::WorkingTree,
            Operand::EmptyTree => Self::EmptyTree,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct FileParams {
    pub(crate) order: Vec<String>,
    /// Repository-relative paths or wildcard patterns; empty selects all changed files.
    pub(crate) paths: Vec<String>,
    pub(crate) renames: bool,
}

impl Default for FileParams {
    fn default() -> Self {
        Self {
            order: Vec::new(),
            paths: Vec::new(),
            renames: true,
        }
    }
}

/// Blob IDs pin revision/index content without retaining every file's source text.
enum Source {
    Absent,
    Blob { id: Oid, mode: git2::FileMode },
    WorkingFile { path: PathBuf, mode: git2::FileMode },
}

impl Source {
    fn from_delta(
        file: git2::DiffFile<'_>,
        operand: &Operand,
        workspace: Option<&Path>,
        absent: bool,
    ) -> Result<Self> {
        if absent {
            return Ok(Self::Absent);
        }
        if matches!(operand, Operand::WorkingTree) {
            return Ok(Self::WorkingFile {
                path: workspace
                    .ok_or("working-tree comparison requires a working tree")?
                    .join(file.path().ok_or("missing file path")?),
                mode: file.mode(),
            });
        }
        Ok(Self::Blob {
            id: file.id(),
            mode: file.mode(),
        })
    }

    fn read(&self, repo: &Repository) -> std::result::Result<String, FileProblem> {
        let mode = match self {
            Self::Absent => return Ok(String::new()),
            Self::Blob { mode, .. } | Self::WorkingFile { mode, .. } => mode,
        };
        if !matches!(mode, git2::FileMode::Blob | git2::FileMode::BlobExecutable) {
            return Err(FileProblem::new(
                "unsupported_file_type",
                "structural diffs currently require regular text files (not symlinks or submodules)",
            ));
        }
        let bytes = match self {
            Self::Blob { id, .. } => repo
                .find_blob(*id)
                .map_err(|error| FileProblem::new("read_failed", error))?
                .content()
                .to_vec(),
            Self::WorkingFile { path, .. } => {
                std::fs::read(path).map_err(|error| FileProblem::new("read_failed", error))?
            }
            Self::Absent => unreachable!(),
        };
        if bytes.contains(&0) {
            return Err(FileProblem::new(
                "binary",
                "structural diffs currently support text files only",
            ));
        }
        String::from_utf8(bytes).map_err(|error| FileProblem::new("not_utf8", error))
    }
}

struct PendingFile {
    file: FileChange,
    before: Source,
    after: Source,
}

pub(crate) struct DiffSession {
    repo: Repository,
    pub(crate) comparison: Comparison,
    params: Arc<Params>,
    files: std::vec::IntoIter<PendingFile>,
    pub(crate) context_lines: u32,
    pub(crate) diff_options: crate::options::DiffOptions,
}

impl DiffSession {
    pub(crate) fn file_manifest(&self) -> Vec<FileChange> {
        self.files
            .as_slice()
            .iter()
            .map(|pending| pending.file.clone())
            .collect()
    }

    pub(crate) fn remaining(&self) -> usize {
        self.files.len()
    }

    pub(crate) fn open(
        workspace: &Path,
        comparison: Comparison,
        params: Arc<Params>,
        files: &FileParams,
    ) -> Result<Self> {
        let repo = Repository::open(workspace)?;
        let comparison = comparison.resolve(&repo)?;
        let pending = {
            let diff = comparison.diff(&repo, files)?;
            let mut pending = Vec::new();
            for delta in diff.deltas() {
                let status = match delta.status() {
                    Delta::Added => FileStatus::Added,
                    Delta::Deleted => FileStatus::Deleted,
                    Delta::Modified => FileStatus::Modified,
                    Delta::Renamed => FileStatus::Renamed,
                    Delta::Typechange => FileStatus::TypeChanged,
                    Delta::Conflicted => FileStatus::Conflicted,
                    _ => continue,
                };
                let path = |file: git2::DiffFile<'_>| -> Result<String> {
                    Ok(file
                        .path()
                        .and_then(Path::to_str)
                        .ok_or("non-UTF-8 Git paths are unsupported")?
                        .to_owned())
                };
                let old_path = if delta.status() == Delta::Added {
                    None
                } else {
                    Some(path(delta.old_file())?)
                };
                let new_path = if delta.status() == Delta::Deleted {
                    None
                } else {
                    Some(path(delta.new_file())?)
                };
                let file_ref = |file: git2::DiffFile<'_>, path: &str| protocol::FileRef {
                    path: path.to_owned(),
                    oid: file.id().to_string(),
                    mode: format!("{:o}", u32::from(file.mode())),
                };
                let sides = match (&old_path, &new_path) {
                    (Some(old), Some(new)) => protocol::Pairing::Both {
                        lhs: file_ref(delta.old_file(), old),
                        rhs: file_ref(delta.new_file(), new),
                    },
                    (Some(old), None) => protocol::Pairing::LeftOnly {
                        lhs: file_ref(delta.old_file(), old),
                    },
                    (None, Some(new)) => protocol::Pairing::RightOnly {
                        rhs: file_ref(delta.new_file(), new),
                    },
                    (None, None) => unreachable!("a delta has a path"),
                };
                let language =
                    language_of(new_path.as_deref().or(old_path.as_deref()).expect("a path"));
                let mut file = FileChange {
                    old_path,
                    new_path,
                    status,
                    class: None,
                    sides,
                    language,
                };
                file.class = category(&repo, file.path())?;
                pending.push(PendingFile {
                    before: Source::from_delta(
                        delta.old_file(),
                        &comparison.before,
                        repo.workdir(),
                        file.old_path.is_none(),
                    )?,
                    after: Source::from_delta(
                        delta.new_file(),
                        &comparison.after,
                        repo.workdir(),
                        file.new_path.is_none(),
                    )?,
                    file,
                });
            }
            let rank = |file: &FileChange| {
                file.class
                    .as_ref()
                    .and_then(|class| files.order.iter().position(|item| item == class))
                    .unwrap_or(files.order.len())
            };
            pending.sort_by(|a, b| {
                rank(&a.file)
                    .cmp(&rank(&b.file))
                    .then_with(|| a.file.path().cmp(b.file.path()))
            });
            pending
        };
        Ok(Self {
            repo,
            comparison,
            params,
            files: pending.into_iter(),
            context_lines: 3,
            diff_options: crate::options::DiffOptions::default(),
        })
    }
}

/// Sources read on the session thread; diffing needs no repository access.
pub(crate) struct LoadedFile {
    pub(crate) file: FileChange,
    before: String,
    after: String,
    pub(crate) params: Arc<Params>,
    pub(crate) context_lines: u32,
    diff_options: crate::options::DiffOptions,
}

impl LoadedFile {
    pub(crate) fn sizes(&self) -> (u64, u64) {
        (self.before.len() as u64, self.after.len() as u64)
    }

    pub(crate) fn diff(&self) -> DiffResult {
        DiffResult::from_sources_with_options(
            self.file.path(),
            &self.before,
            &self.after,
            &self.params,
            &crate::options::DisplayOptions {
                num_context_lines: self.context_lines,
                ..Default::default()
            },
            &self.diff_options,
        )
    }
}

impl DiffSession {
    /// Read the next file's sources without diffing them.
    pub(crate) fn load(
        &mut self,
    ) -> Option<(FileChange, std::result::Result<LoadedFile, FileProblem>)> {
        let pending = self.files.next()?;
        let result = (|| {
            if matches!(pending.file.status, FileStatus::Conflicted) {
                return Err(FileProblem::new(
                    "unmerged",
                    "unmerged index entry: resolve the conflict before requesting a structural diff",
                ));
            }
            Ok(LoadedFile {
                before: pending.before.read(&self.repo)?,
                after: pending.after.read(&self.repo)?,
                file: pending.file.clone(),
                params: Arc::clone(&self.params),
                context_lines: self.context_lines,
                diff_options: self.diff_options.clone(),
            })
        })();
        Some((pending.file, result))
    }
}

impl Iterator for DiffSession {
    type Item = (FileChange, Result<DiffResult>);
    fn next(&mut self) -> Option<Self::Item> {
        let (file, loaded) = self.load()?;
        Some((
            file,
            loaded
                .map(|loaded| loaded.diff())
                .map_err(|problem| Box::new(problem) as _),
        ))
    }
}
