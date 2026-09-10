//! Git comparison selection and lazy source loading used by the CLI and its stdout stream.
use crate::config::Params;
use crate::summary::DiffResult;
use git2::{AttrCheckFlags, AttrValue, Delta, Diff, DiffFindOptions, DiffOptions, Oid, Repository};
use serde::{Deserialize, Serialize};
use std::{
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
}

impl FileChange {
    pub(crate) fn path(&self) -> &str {
        self.new_path
            .as_deref()
            .or(self.old_path.as_deref())
            .expect("changed file has a path")
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

    fn read(&self, repo: &Repository) -> Result<String> {
        let mode = match self {
            Self::Absent => return Ok(String::new()),
            Self::Blob { mode, .. } | Self::WorkingFile { mode, .. } => mode,
        };
        if !matches!(mode, git2::FileMode::Blob | git2::FileMode::BlobExecutable) {
            return Err("structural diffs currently require regular text files (not symlinks or submodules)".into());
        }
        let bytes = match self {
            Self::Blob { id, .. } => repo.find_blob(*id)?.content().to_vec(),
            Self::WorkingFile { path, .. } => std::fs::read(path)?,
            Self::Absent => unreachable!(),
        };
        if bytes.contains(&0) {
            return Err("structural diffs currently support text files only".into());
        }
        Ok(String::from_utf8(bytes)?)
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
                let mut file = FileChange {
                    old_path: if delta.status() == Delta::Added {
                        None
                    } else {
                        Some(path(delta.old_file())?)
                    },
                    new_path: if delta.status() == Delta::Deleted {
                        None
                    } else {
                        Some(path(delta.new_file())?)
                    },
                    status,
                    class: None,
                };
                file.class = match AttrValue::from_string(repo.get_attr(
                    Path::new(file.path()),
                    "diffr-classify",
                    AttrCheckFlags::FILE_THEN_INDEX,
                )?) {
                    AttrValue::String(value) => Some(value.to_owned()),
                    _ => None,
                };
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

impl Iterator for DiffSession {
    type Item = (FileChange, Result<DiffResult>);
    fn next(&mut self) -> Option<Self::Item> {
        let pending = self.files.next()?;
        let result = (|| {
            if matches!(pending.file.status, FileStatus::Conflicted) {
                return Err("unmerged index entry: resolve the conflict before requesting a structural diff".into());
            }
            let before = pending.before.read(&self.repo)?;
            let after = pending.after.read(&self.repo)?;
            Ok(DiffResult::from_sources_with_options(
                pending.file.path(),
                &before,
                &after,
                &self.params,
                &crate::options::DisplayOptions {
                    num_context_lines: self.context_lines,
                    ..Default::default()
                },
                &self.diff_options,
            ))
        })();
        Some((pending.file, result))
    }
}
