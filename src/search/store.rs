//! Computed diffs, cached as one JSON file per key.
use crate::{
    pairing::Pairing,
    protocol::{FileChange, Source, Stats},
};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, io::Write, path::PathBuf};

/// Bump when diff algorithms, parsers, or serialized tree semantics change.
const CACHE_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), ":search-diff-v6");

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub(crate) struct DiffKey(String);
impl DiffKey {
    /// Hash a canonical JSON description of analysis settings and source identities.
    pub(crate) fn new(identity: &impl Serialize) -> Result<Self> {
        let mut canonical = serde_json::to_value((CACHE_VERSION, identity))?;
        canonical.sort_all_objects();
        Ok(Self(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&canonical)?)
        )))
    }
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for DiffKey {
    type Error = anyhow::Error;
    fn try_from(value: String) -> Result<Self> {
        ensure!(
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid diff key"
        );
        Ok(Self(value))
    }
}
impl From<DiffKey> for String {
    fn from(key: DiffKey) -> Self {
        key.0
    }
}

/// A computed diff, without search highlights.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StoredDiff {
    pub(crate) entry: FileChange,
    pub(crate) sources: Pairing<Source>,
    pub(crate) stats: Stats,
    pub(crate) hidden: Option<String>,
}

pub(crate) struct FileStore {
    directory: PathBuf,
}
#[derive(Serialize, Deserialize)]
struct Record {
    key: DiffKey,
    diff: StoredDiff,
}
impl FileStore {
    pub(crate) fn open(directory: impl Into<PathBuf>) -> Result<Self> {
        let directory = directory.into();
        fs::create_dir_all(&directory).context("create diff store directory")?;
        Ok(Self { directory })
    }
    fn path(&self, key: &DiffKey) -> PathBuf {
        self.directory.join(format!("{}.json", key.as_str()))
    }
    pub(crate) fn get(&self, key: &DiffKey) -> Result<Option<StoredDiff>> {
        let bytes = match fs::read(self.path(key)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("read diff store entry"),
        };
        let record: Record = serde_json::from_slice(&bytes).context("decode diff store entry")?;
        ensure!(&record.key == key, "diff store key mismatch");
        Ok(Some(record.diff))
    }
    /// Write through a temporary file, so a reader never sees half an entry.
    pub(crate) fn put(&self, key: &DiffKey, diff: &StoredDiff) -> Result<()> {
        let mut file = tempfile::NamedTempFile::new_in(&self.directory)?;
        serde_json::to_writer(
            &mut file,
            &Record {
                key: key.clone(),
                diff: diff.clone(),
            },
        )?;
        file.flush()?;
        file.as_file().sync_all()?;
        file.persist(self.path(key))
            .context("publish diff store entry")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{FileRef, FileStatus, LineCounts, Region};

    fn sample(text: &str) -> StoredDiff {
        StoredDiff {
            hidden: None,
            stats: Stats {
                textual: LineCounts {
                    added: 1,
                    removed: 0,
                },
                visible: LineCounts {
                    added: 1,
                    removed: 0,
                },
                fallback: None,
            },
            entry: FileChange {
                file: Pairing::RightOnly {
                    rhs: FileRef {
                        path: "x.rs".into(),
                        oid: "abc".into(),
                        mode: "100644".into(),
                    },
                },
                status: FileStatus::Added,
                tags: vec![],
            },
            sources: Pairing::RightOnly {
                rhs: Source {
                    text: text.into(),
                    syntax: vec![],
                    root: Region::root(1, vec![]),
                },
            },
        }
    }
    fn text(diff: Option<StoredDiff>) -> String {
        let Pairing::RightOnly { rhs } = diff.unwrap().sources else {
            panic!("one side");
        };
        rhs.text
    }

    #[test]
    fn entries_survive_a_reopen_and_corruption_is_an_error() {
        let directory = tempfile::tempdir().unwrap();
        let key = DiffKey::new(&("a.rs", "blob", "config")).unwrap();
        let store = FileStore::open(directory.path()).unwrap();
        assert!(store.get(&key).unwrap().is_none());
        store.put(&key, &sample("saved")).unwrap();
        store.put(&key, &sample("replaced")).unwrap();
        let reopened = FileStore::open(directory.path()).unwrap();
        assert_eq!(text(reopened.get(&key).unwrap()), "replaced");
        fs::write(reopened.path(&key), b"broken JSON").unwrap();
        assert!(reopened.get(&key).is_err());
    }
}
