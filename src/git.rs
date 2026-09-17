//! Git comparison selection and lazy source loading used by the CLI and its stdout stream.
use crate::config::Params;
use crate::pairing::Pairing;
use crate::plugin::Pipeline;
use crate::protocol;
use crate::summary::{DiffResult, FileContent, FileFormat};
use crate::tags::{self, Attributes, Prefix, PREFIX_BYTES};
use anyhow::Context as _;
use git2::{Delta, Diff, DiffFindOptions, DiffOptions, Oid, Repository};
use serde::Deserialize;
use std::{
    fmt,
    io::Read as _,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Clone, Debug, Deserialize)]
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

#[derive(Clone, Debug)]
pub(crate) enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    TypeChanged,
    Conflicted,
}

#[derive(Clone, Debug)]
pub(crate) struct FileChange {
    pub(crate) old_path: Option<String>,
    pub(crate) new_path: Option<String>,
    pub(crate) status: FileStatus,
    /// Sorted and deduplicated; see [`crate::tags`].
    pub(crate) tags: Vec<String>,
    /// Git's delta sides.
    pub(crate) sides: Pairing<protocol::FileRef>,
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
                Pairing::Both {
                    lhs: file_ref(old),
                    rhs: file_ref(new),
                },
            ),
            (Some(old), None) => (
                FileStatus::Deleted,
                Pairing::LeftOnly { lhs: file_ref(old) },
            ),
            (None, Some(new)) => (FileStatus::Added, Pairing::RightOnly { rhs: file_ref(new) }),
            (None, None) => panic!("a standalone comparison needs at least one path"),
        };
        Self {
            old_path,
            new_path,
            status,
            // Paths outside a repository have no attributes, and Linguist's
            // rules are written for repository-relative paths.
            tags: Vec::new(),
            sides,
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
            tags: self.tags.clone(),
        }
    }
}

/// Why one file could not be diffed. Loading attaches it to the error, and
/// the stream turns it into the record's `code`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FileError {
    UnsupportedFileType,
    ReadFailed,
    NotUtf8,
    Unmerged,
}

impl FileError {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::UnsupportedFileType => "unsupported_file_type",
            Self::ReadFailed => "read_failed",
            Self::NotUtf8 => "not_utf8",
            Self::Unmerged => "unmerged",
        }
    }
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsupportedFileType => {
                "structural diffs currently require regular text files (not symlinks or submodules)"
            }
            Self::ReadFailed => "could not read the source",
            Self::NotUtf8 => "the source is not valid UTF-8",
            Self::Unmerged => {
                "unmerged index entry: resolve the conflict before requesting a structural diff"
            }
        })
    }
}

impl std::error::Error for FileError {}

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
#[derive(Clone)]
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

    /// The start of the source for the content rules, or `None` when they
    /// cannot apply: not a regular file, binary, or not UTF-8. Loading the
    /// whole source handles those cases.
    fn prefix(&self, repo: &Repository) -> anyhow::Result<Option<(Vec<u8>, bool)>> {
        let Some(mut bytes) = self.head(repo, PREFIX_BYTES + 1)? else {
            return Ok(None);
        };
        let complete = bytes.len() <= PREFIX_BYTES;
        bytes.truncate(PREFIX_BYTES);
        if bytes.contains(&0) {
            return Ok(None);
        }
        Ok(Some((bytes, complete)))
    }

    /// Up to `max` bytes from the start of the source, or `None` when it is
    /// absent or not a regular file.
    fn head(&self, repo: &Repository, max: usize) -> anyhow::Result<Option<Vec<u8>>> {
        match self {
            Self::Absent => Ok(None),
            Self::Blob { mode, .. } | Self::WorkingFile { mode, .. }
                if !matches!(mode, git2::FileMode::Blob | git2::FileMode::BlobExecutable) =>
            {
                Ok(None)
            }
            Self::Blob { id, .. } => {
                let blob = repo.find_blob(*id).context(FileError::ReadFailed)?;
                let content = blob.content();
                Ok(Some(content[..content.len().min(max)].to_vec()))
            }
            Self::WorkingFile { path, .. } => {
                let mut bytes = Vec::with_capacity(max.min(PREFIX_BYTES + 1));
                std::fs::File::open(path)
                    .context(FileError::ReadFailed)?
                    .take(max as u64)
                    .read_to_end(&mut bytes)
                    .context(FileError::ReadFailed)?;
                Ok(Some(bytes))
            }
        }
    }

    fn read(&self, repo: &Repository) -> anyhow::Result<Vec<u8>> {
        let mode = match self {
            Self::Absent => return Ok(Vec::new()),
            Self::Blob { mode, .. } | Self::WorkingFile { mode, .. } => mode,
        };
        if !matches!(mode, git2::FileMode::Blob | git2::FileMode::BlobExecutable) {
            return Err(FileError::UnsupportedFileType.into());
        }
        let bytes = match self {
            Self::Blob { id, .. } => repo
                .find_blob(*id)
                .context(FileError::ReadFailed)?
                .content()
                .to_vec(),
            Self::WorkingFile { path, .. } => std::fs::read(path).context(FileError::ReadFailed)?,
            Self::Absent => unreachable!(),
        };
        Ok(bytes)
    }
}

struct PendingFile {
    file: FileChange,
    before: Source,
    after: Source,
    /// Reading the start of the file for its tags failed; loading reports it
    /// as this file's error.
    prefix_error: Option<anyhow::Error>,
}

/// Whether Linguist's content rules call these bytes generated. A prefix cut
/// inside a UTF-8 sequence drops the partial character; bytes that are not
/// UTF-8 otherwise are not text, and no content rule applies.
fn generated_by_content(path: &str, bytes: &[u8], complete: bool) -> bool {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) if !complete && error.error_len().is_none() => {
            std::str::from_utf8(&bytes[..error.valid_up_to()]).expect("valid up to here")
        }
        Err(_) => return false,
    };
    tags::generated_by_content(path, &Prefix { text, complete })
}

pub(crate) struct DiffSession {
    repo: Repository,
    pub(crate) comparison: Comparison,
    params: Arc<Params>,
    files: std::vec::IntoIter<PendingFile>,
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

    /// List the comparison's files and their tags: the bundled rules, git
    /// attributes, then each plugin's `classify` in `pipeline`, whose
    /// failure fails the session.
    pub(crate) fn open(
        workspace: &Path,
        comparison: Comparison,
        params: Arc<Params>,
        files: &FileParams,
        pipeline: &Pipeline,
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
                    (Some(old), Some(new)) => Pairing::Both {
                        lhs: file_ref(delta.old_file(), old),
                        rhs: file_ref(delta.new_file(), new),
                    },
                    (Some(old), None) => Pairing::LeftOnly {
                        lhs: file_ref(delta.old_file(), old),
                    },
                    (None, Some(new)) => Pairing::RightOnly {
                        rhs: file_ref(delta.new_file(), new),
                    },
                    (None, None) => unreachable!("a delta has a path"),
                };
                let mut file = FileChange {
                    old_path,
                    new_path,
                    status,
                    tags: Vec::new(),
                    sides,
                };
                let before = Source::from_delta(
                    delta.old_file(),
                    &comparison.before,
                    repo.workdir(),
                    file.old_path.is_none(),
                )?;
                let after = Source::from_delta(
                    delta.new_file(),
                    &comparison.after,
                    repo.workdir(),
                    file.new_path.is_none(),
                )?;
                let path = file.path();
                let attributes = Attributes::lookup(&repo, path)?;
                let mut bundled = tags::from_path(path);
                let mut prefix_error = None;
                // Content is read only when it could change the answer.
                if attributes.generated.is_none()
                    && !bundled.contains(tags::GENERATED)
                    && tags::needs_content(path)
                {
                    let side = match &file.new_path {
                        Some(_) => &after,
                        None => &before,
                    };
                    match side.prefix(&repo) {
                        Ok(Some((bytes, complete))) => {
                            if generated_by_content(path, &bytes, complete) {
                                bundled.insert(tags::GENERATED);
                            }
                        }
                        Ok(None) => {}
                        Err(error) => prefix_error = Some(error),
                    }
                }
                file.tags = attributes.resolve(bundled);
                file.tags = pipeline
                    .classify(&file.manifest_entry())
                    .map_err(|error| format!("{error:#}"))?;
                pending.push(PendingFile {
                    before,
                    after,
                    prefix_error,
                    file,
                });
            }
            // A file ranks by the earliest `--order` tag it carries.
            let rank = |file: &FileChange| {
                files
                    .order
                    .iter()
                    .position(|tag| file.tags.contains(tag))
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
            diff_options: crate::options::DiffOptions::default(),
        })
    }
}

/// Sources read on the session thread; diffing needs no repository access.
pub(crate) struct LoadedFile {
    pub(crate) file: FileChange,
    before: Vec<u8>,
    after: Vec<u8>,
    pub(crate) params: Arc<Params>,
    diff_options: crate::options::DiffOptions,
}

impl LoadedFile {
    pub(crate) fn sizes(&self) -> (u64, u64) {
        (self.before.len() as u64, self.after.len() as u64)
    }

    /// A fold query conflict fails this file alone.
    pub(crate) fn diff(&self) -> anyhow::Result<DiffResult> {
        // Preserve binary files as successful, size-only records. Rejecting
        // them while loading bypasses the protocol and TUI's binary support.
        if self.before.contains(&0) || self.after.contains(&0) {
            return Ok(DiffResult {
                file_format: FileFormat::Binary,
                lhs_src: FileContent::Binary,
                rhs_src: FileContent::Binary,
                lhs_positions: vec![],
                rhs_positions: vec![],
                lhs_folds: vec![],
                rhs_folds: vec![],
            });
        }
        let before = std::str::from_utf8(&self.before).context(FileError::NotUtf8)?;
        let after = std::str::from_utf8(&self.after).context(FileError::NotUtf8)?;
        let options = crate::options::DiffOptions {
            generated: self.file.tags.iter().any(|tag| tag == tags::GENERATED),
            ..self.diff_options.clone()
        };
        Ok(DiffResult::from_sources_with_options(
            self.file.path(),
            before,
            after,
            &self.params,
            &options,
        )?)
    }
}

impl DiffSession {
    /// Read the next file's sources without diffing them.
    pub(crate) fn load(&mut self) -> Option<(FileChange, anyhow::Result<LoadedFile>)> {
        let mut pending = self.files.next()?;
        let prefix_error = pending.prefix_error.take();
        let result = (|| {
            if let Some(error) = prefix_error {
                return Err(error);
            }
            if matches!(pending.file.status, FileStatus::Conflicted) {
                return Err(FileError::Unmerged.into());
            }
            Ok(LoadedFile {
                before: pending.before.read(&self.repo)?,
                after: pending.after.read(&self.repo)?,
                file: pending.file.clone(),
                params: Arc::clone(&self.params),
                diff_options: self.diff_options.clone(),
            })
        })();
        Some((pending.file, result))
    }
}
