//! Git source loading shared by CLI and streaming callers.
use git2::{Commit, ErrorCode, Repository};
use std::path::Path;

pub(crate) type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub(crate) fn read_blob(
    repo: &Repository,
    commit: &Commit<'_>,
    path: &str,
) -> Result<Option<String>> {
    let tree = commit.tree()?;
    let entry = match tree.get_path(Path::new(path)) {
        Ok(entry) => entry,
        Err(error) if error.code() == ErrorCode::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !matches!(entry.filemode(), 0o100644 | 0o100755) {
        return Err(
            "Review v0 requires a regular file path, not a directory, symlink or submodule".into(),
        );
    }
    let blob = repo.find_blob(entry.id())?;
    let bytes = blob.content();
    if bytes.contains(&0) {
        return Err("Review v0 supports text files only".into());
    }
    Ok(Some(std::str::from_utf8(bytes)?.to_owned()))
}

use crate::config::Params;
use crate::summary::DiffResult;
use git2::{AttrCheckFlags, AttrValue, Delta, DiffFindOptions, DiffOptions, Oid};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FileStatus {
    Unchanged,
    Added,
    Deleted,
    Modified,
    Renamed,
    TypeChanged,
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

/// Client selection and ordering; omitted order leaves files in path order.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct FileParams {
    pub(crate) order: Vec<String>,
    /// Exact repository-relative paths; empty selects all changed files.
    pub(crate) paths: Vec<String>,
}

/// Owns pinned revisions and descriptors, not precomputed patches.
pub(crate) struct DiffSession {
    repo: Repository,
    pub(crate) base: Oid,
    pub(crate) head: Oid,
    params: Arc<Params>,
    files: std::vec::IntoIter<FileChange>,
}

impl DiffSession {
    pub(crate) fn remaining(&self) -> usize {
        self.files.len()
    }

    pub(crate) fn open(
        workspace: &Path,
        base: &str,
        head: &str,
        params: Arc<Params>,
        files: &FileParams,
    ) -> Result<Self> {
        let repo = Repository::open(workspace)?;
        let base = repo.revparse_single(base)?.peel_to_commit()?.id();
        let head = repo.revparse_single(head)?.peel_to_commit()?.id();
        let files = discover(&repo, base, head, &files.order, &files.paths)?;
        Ok(Self {
            repo,
            base,
            head,
            params,
            files: files.into_iter(),
        })
    }
}

impl Iterator for DiffSession {
    type Item = (FileChange, Result<DiffResult>);

    fn next(&mut self) -> Option<Self::Item> {
        let file = self.files.next()?;
        let result = (|| {
            let base = self.repo.find_commit(self.base)?;
            let head = self.repo.find_commit(self.head)?;
            let lhs = file
                .old_path
                .as_deref()
                .map(|path| read_blob(&self.repo, &base, path))
                .transpose()?
                .flatten();
            let rhs = file
                .new_path
                .as_deref()
                .map(|path| read_blob(&self.repo, &head, path))
                .transpose()?
                .flatten();
            Ok(DiffResult::from_sources_with_params(
                file.path(),
                lhs.as_deref().unwrap_or_default(),
                rhs.as_deref().unwrap_or_default(),
                &self.params,
            ))
        })();
        Some((file, result))
    }
}

fn discover(
    repo: &Repository,
    base: Oid,
    head: Oid,
    order: &[String],
    paths: &[String],
) -> Result<Vec<FileChange>> {
    let base = repo.find_commit(base)?.tree()?;
    let head = repo.find_commit(head)?.tree()?;
    let mut diff = repo.diff_tree_to_tree(
        Some(&base),
        Some(&head),
        Some(DiffOptions::new().include_typechange(true)),
    )?;
    diff.find_similar(Some(DiffFindOptions::new().renames(true)))?;
    let mut files = Vec::new();
    for delta in diff.deltas() {
        let status = match delta.status() {
            Delta::Added => FileStatus::Added,
            Delta::Deleted => FileStatus::Deleted,
            Delta::Modified => FileStatus::Modified,
            Delta::Renamed => FileStatus::Renamed,
            Delta::Typechange => FileStatus::TypeChanged,
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
        if !paths.is_empty()
            && !paths
                .iter()
                .any(|path| old_path.as_ref() == Some(path) || new_path.as_ref() == Some(path))
        {
            continue;
        }
        files.push(FileChange {
            old_path,
            new_path,
            status,
            class: None,
        });
    }
    // An explicitly selected unchanged file is still a valid single-file diff.
    for path in paths {
        if path.is_empty()
            || Path::new(path)
                .components()
                .any(|part| !matches!(part, std::path::Component::Normal(_)))
        {
            return Err("expected a repository-relative path".into());
        }
        if files.iter().any(|file| {
            file.old_path.as_ref() == Some(path) || file.new_path.as_ref() == Some(path)
        }) {
            continue;
        }
        base.get_path(Path::new(path))?;
        head.get_path(Path::new(path))?;
        files.push(FileChange {
            old_path: Some(path.clone()),
            new_path: Some(path.clone()),
            status: FileStatus::Unchanged,
            class: None,
        });
    }
    for file in &mut files {
        let attr = repo.get_attr(
            Path::new(file.path()),
            "diffr-classify",
            AttrCheckFlags::FILE_THEN_INDEX,
        )?;
        file.class = match AttrValue::from_string(attr) {
            AttrValue::String(value) => Some(value.to_owned()),
            _ => None,
        };
    }
    let rank = |file: &FileChange| {
        file.class
            .as_ref()
            .and_then(|class| order.iter().position(|item| item == class))
            .unwrap_or(order.len())
    };
    files.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.path().cmp(b.path())));
    Ok(files)
}
