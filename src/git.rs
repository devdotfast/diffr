//! Git comparison selection and lazy source loading used by the CLI and its stdout stream.
use crate::config::Params;
use crate::pairing::Pairing;
use crate::plugin::Pipeline;
use crate::protocol;
use crate::summary::{DiffResult, FallbackCause, FileContent, FileFormat};
use crate::tags;
use anyhow::Context as _;
use gix::filter::plumbing::pipeline::convert::ToGitOutcome;
use gix::{index::entry::Mode, ObjectId as Oid, Repository};
mod diff;
pub(crate) use diff::Diff;
use serde::Deserialize;
use std::{fmt, io::Read as _, path::Path, sync::Arc};

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
                let object = repo.rev_parse_single(r#ref.as_str())?.object()?;
                // Retain commit identity where possible, while accepting tree objects too.
                let id = match object.clone().peel_to_commit() {
                    Ok(commit) => commit.id(),
                    Err(_) => object.peel_to_tree()?.id(),
                };
                Ok(Self::revision(id.to_string()))
            }
            _ => Ok(self.clone()),
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

    pub(crate) fn diff(&self, repo: &Repository, files: &FileParams) -> Result<Diff> {
        diff::compare(repo, self, files)
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
    /// Sorted and deduplicated, from the classifier.
    pub(crate) tags: Vec<String>,
    /// The classifier hid the file behind this reason: it is diffed by line,
    /// not shaped, and shown collapsed.
    pub(crate) hidden: Option<String>,
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
            hidden: None,
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
    /// Repository-relative paths or wildcard patterns; empty selects all changed files.
    pub(crate) paths: Vec<String>,
    pub(crate) renames: bool,
}

impl Default for FileParams {
    fn default() -> Self {
        Self {
            paths: Vec::new(),
            renames: true,
        }
    }
}

/// Blob IDs pin revision/index content without retaining every file's source text.
#[derive(Clone)]
enum Source {
    Absent,
    Blob {
        id: Oid,
        mode: Mode,
    },
    /// A working-tree file, by its repository-relative path.
    WorkingFile {
        path: String,
        mode: Mode,
    },
}

impl Source {
    fn from_entry(
        entry: Option<&diff::Entry>,
        operand: &Operand,
        workspace: Option<&Path>,
    ) -> Result<Self> {
        let Some(entry) = entry else {
            return Ok(Self::Absent);
        };
        if matches!(operand, Operand::WorkingTree) {
            if workspace.is_none() {
                return Err("working-tree comparison requires a working tree".into());
            }
            return Ok(Self::WorkingFile {
                path: entry.path.clone(),
                mode: entry.mode,
            });
        }
        Ok(Self::Blob {
            id: entry.id,
            mode: entry.mode,
        })
    }

    fn read(&self, repo: &Repository) -> anyhow::Result<Vec<u8>> {
        let mode = match self {
            Self::Absent => return Ok(Vec::new()),
            Self::Blob { mode, .. } | Self::WorkingFile { mode, .. } => mode,
        };
        if !matches!(*mode, Mode::FILE | Mode::FILE_EXECUTABLE) {
            return Err(FileError::UnsupportedFileType.into());
        }
        let bytes = match self {
            Self::Blob { id, .. } => {
                repo.find_blob(*id)
                    .context(FileError::ReadFailed)?
                    .detach()
                    .data
            }
            Self::WorkingFile { path, .. } => clean(repo, path).context(FileError::ReadFailed)?,
            Self::Absent => unreachable!(),
        };
        Ok(bytes)
    }
}

/// A working-tree file as Git would store it: through the clean filters
/// (`core.autocrlf`, `eol`, `ident`, filter drivers) its attributes select,
/// as `git diff` reads it.
fn clean(repo: &Repository, path: &str) -> anyhow::Result<Vec<u8>> {
    let workdir = repo
        .workdir()
        .context("working-tree comparison requires a working tree")?;
    let file = std::fs::File::open(workdir.join(path))?;
    let (mut pipeline, index) = repo.filter_pipeline(None)?;
    let mut bytes = Vec::new();
    match pipeline.convert_to_git(file, Path::new(path), &index)? {
        ToGitOutcome::Unchanged(mut file) => {
            file.read_to_end(&mut bytes)?;
        }
        ToGitOutcome::Process(mut output) => {
            output.read_to_end(&mut bytes)?;
        }
        ToGitOutcome::Buffer(buffer) => bytes.extend_from_slice(buffer),
    }
    Ok(bytes)
}

pub(crate) struct PendingFile {
    pub(crate) file: FileChange,
    before: Source,
    after: Source,
}

pub(crate) struct DiffSession {
    repo: gix::ThreadSafeRepository,
    pub(crate) comparison: Comparison,
    params: Arc<Params>,
    files: std::vec::IntoIter<PendingFile>,
    diff_options: crate::options::DiffOptions,
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

    /// List the comparison's files and their tags, from the classifier in
    /// `pipeline`, whose failure fails the session.
    pub(crate) fn open(
        workspace: &Path,
        comparison: Comparison,
        params: Arc<Params>,
        diff_options: crate::options::DiffOptions,
        files: &FileParams,
        pipeline: &Pipeline,
    ) -> Result<Self> {
        let repo = gix::open(workspace)?;
        let comparison = comparison.resolve(&repo)?;
        let pending = {
            let diff = comparison.diff(&repo, files)?;
            let mut pending = Vec::new();
            for delta in &diff.changes {
                let old_path = delta.before.as_ref().map(|entry| entry.path.clone());
                let new_path = delta.after.as_ref().map(|entry| entry.path.clone());
                // A working-tree side names no stored blob, so it carries the
                // null id, as `git diff --raw` writes it.
                let file_ref = |entry: &diff::Entry, operand: &Operand| protocol::FileRef {
                    path: entry.path.clone(),
                    oid: match operand {
                        Operand::WorkingTree => gix::ObjectId::null(repo.object_hash()).to_string(),
                        _ => entry.id.to_string(),
                    },
                    mode: format!("{:o}", entry.mode.bits()),
                };
                let (lhs, rhs) = (&comparison.before, &comparison.after);
                let sides = match (&delta.before, &delta.after) {
                    (Some(old), Some(new)) => Pairing::Both {
                        lhs: file_ref(old, lhs),
                        rhs: file_ref(new, rhs),
                    },
                    (Some(old), None) => Pairing::LeftOnly {
                        lhs: file_ref(old, lhs),
                    },
                    (None, Some(new)) => Pairing::RightOnly {
                        rhs: file_ref(new, rhs),
                    },
                    (None, None) => unreachable!("a change has a path"),
                };
                let file = FileChange {
                    old_path,
                    new_path,
                    status: delta.status.clone(),
                    tags: Vec::new(),
                    hidden: None,
                    sides,
                };
                let before =
                    Source::from_entry(delta.before.as_ref(), &comparison.before, repo.workdir())?;
                let after =
                    Source::from_entry(delta.after.as_ref(), &comparison.after, repo.workdir())?;
                pending.push(PendingFile {
                    before,
                    after,
                    file,
                });
            }
            let entries: Vec<protocol::FileChange> = pending
                .iter()
                .map(|pending| pending.file.manifest_entry())
                .collect();
            let classified = pipeline
                .classify(&entries)
                .map_err(|error| format!("{error:#}"))?;
            for (pending, classified) in pending.iter_mut().zip(classified) {
                pending.file.tags = classified.tags;
                pending.file.hidden = classified.hidden;
            }
            pending
        };
        Ok(Self {
            repo: repo.into_sync(),
            comparison,
            params,
            files: pending.into_iter(),
            diff_options,
        })
    }
}

/// Owned source bytes; diffing needs no repository access.
pub(crate) struct LoadedFile {
    pub(crate) file: FileChange,
    before: Vec<u8>,
    after: Vec<u8>,
}

impl LoadedFile {
    pub(crate) fn sizes(&self) -> (u64, u64) {
        (self.before.len() as u64, self.after.len() as u64)
    }
}

impl DiffSession {
    /// Assign concrete files before any task is scheduled.
    pub(crate) fn into_files(self) -> (Differ, std::vec::IntoIter<PendingFile>) {
        (
            Differ {
                repo: self.repo,
                params: self.params,
                diff_options: self.diff_options,
            },
            self.files,
        )
    }
}

/// What every file's load and diff share: the repository, each blocking task
/// making its own gix view of it, the compiled queries and the diff options.
pub(crate) struct Differ {
    repo: gix::ThreadSafeRepository,
    pub(crate) params: Arc<Params>,
    diff_options: crate::options::DiffOptions,
}

impl Differ {
    pub(crate) fn load(&self, pending: PendingFile) -> anyhow::Result<LoadedFile> {
        if matches!(pending.file.status, FileStatus::Conflicted) {
            return Err(FileError::Unmerged.into());
        }
        let repo = self.repo.to_thread_local();
        Ok(LoadedFile {
            before: pending.before.read(&repo)?,
            after: pending.after.read(&repo)?,
            file: pending.file,
        })
    }

    /// A fold query conflict fails this file alone.
    pub(crate) fn diff(&self, loaded: &LoadedFile) -> anyhow::Result<DiffResult> {
        // Preserve binary files as successful, size-only records. Rejecting
        // them while loading bypasses the protocol and TUI's binary support.
        if loaded.before.contains(&0) || loaded.after.contains(&0) {
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
        let before = std::str::from_utf8(&loaded.before).context(FileError::NotUtf8)?;
        let after = std::str::from_utf8(&loaded.after).context(FileError::NotUtf8)?;
        let file = &loaded.file;
        let options = crate::options::DiffOptions {
            by_line: if file.tags.iter().any(|tag| tag == tags::GENERATED) {
                Some(FallbackCause::Generated)
            } else if file.hidden.is_some() {
                Some(FallbackCause::Hidden)
            } else {
                None
            },
            ..self.diff_options.clone()
        };
        Ok(DiffResult::from_sources_with_options(
            file.path(),
            before,
            after,
            &self.params,
            &options,
        )?)
    }
}
