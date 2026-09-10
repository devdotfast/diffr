//! Git source loading shared by CLI and streaming callers.
use git2::{Commit, ErrorCode, Repository};
use std::path::Path;

pub(super) type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub(super) fn read_blob(
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
