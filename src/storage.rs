//! Built-in computed-diff stores and host configuration.
mod file;
mod memory;
mod sqlite;
use crate::search::store::Store;
use anyhow::Result;
pub use file::FileStore;
pub use memory::MemoryStore;
use serde::{Deserialize, Serialize};
pub use sqlite::SqliteStore;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

/// Host-owned storage settings, loaded through diffr config.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct StoreConfig {
    /// Where computed diffs are kept. Memory lasts for the process; file and SQLite persist.
    #[schemars(title = "Diff storage backend", extend("x-group" = "Storage"))]
    pub(crate) backend: StoreBackend,
    /// Storage directory, relative to the source repository unless absolute. SQLite uses diffs.sqlite inside it.
    #[schemars(title = "Diff storage directory", extend("x-group" = "Storage"))]
    pub(crate) path: PathBuf,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub(crate) enum StoreBackend {
    #[default]
    Memory,
    File,
    Sqlite,
}
impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            backend: StoreBackend::Memory,
            path: ".cache/diffr".into(),
        }
    }
}
impl StoreConfig {
    pub(crate) fn open(&self, repository: &Path) -> Result<Arc<dyn Store>> {
        let directory = repository.join(&self.path);
        Ok(match self.backend {
            StoreBackend::Memory => Arc::new(MemoryStore::default()),
            StoreBackend::File => Arc::new(FileStore::open(directory)?),
            StoreBackend::Sqlite => Arc::new(SqliteStore::open(directory.join("diffs.sqlite"))?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{FileChange, FileRef, FileStatus};
    use crate::{
        pairing::Pairing,
        protocol::Source,
        search::store::{DiffKey, StoredDiff},
    };
    use rusqlite::Connection;
    use std::fs;

    fn sample(text: &str) -> StoredDiff {
        StoredDiff {
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
                    regions: vec![],
                },
            },
        }
    }
    fn contract(store: &dyn Store) {
        let key = DiffKey::new(&("a.rs", "blob", "config")).unwrap();
        let other = DiffKey::new(&("a.rs", "blob2", "config")).unwrap();
        assert!(store.get(&key).unwrap().is_none());
        let value = sample("hello\n");
        store.put(&key, &value).unwrap();
        assert_eq!(
            serde_json::to_value(store.get(&key).unwrap().unwrap()).unwrap(),
            serde_json::to_value(value).unwrap()
        );
        assert!(store.get(&other).unwrap().is_none());
        store.put(&key, &sample("replacement")).unwrap();
        assert_eq!(
            serde_json::to_value(store.get(&key).unwrap().unwrap()).unwrap(),
            serde_json::to_value(sample("replacement")).unwrap()
        );
        store.remove(&key).unwrap();
        store.remove(&key).unwrap();
        assert!(store.get(&key).unwrap().is_none());
    }
    #[test]
    fn all_backends_obey_the_same_contract() {
        let directory = tempfile::tempdir().unwrap();
        contract(&MemoryStore::default());
        contract(&FileStore::open(directory.path().join("files")).unwrap());
        contract(&SqliteStore::open(directory.path().join("diffs.sqlite")).unwrap());
    }
    #[test]
    fn durable_stores_reopen_and_report_corruption() {
        let directory = tempfile::tempdir().unwrap();
        let key = DiffKey::new(&"identity").unwrap();
        let files = directory.path().join("files");
        FileStore::open(&files)
            .unwrap()
            .put(&key, &sample("saved"))
            .unwrap();
        let reopened = FileStore::open(&files).unwrap();
        assert!(reopened.get(&key).unwrap().is_some());
        fs::write(files.join(format!("{}.json", key.as_str())), b"broken JSON").unwrap();
        assert!(reopened.get(&key).is_err());
        let db = directory.path().join("diffs.sqlite");
        SqliteStore::open(&db)
            .unwrap()
            .put(&key, &sample("saved"))
            .unwrap();
        assert!(SqliteStore::open(&db).unwrap().get(&key).unwrap().is_some());
        Connection::open(&db)
            .unwrap()
            .execute("UPDATE diffs SET data = ?1", [b"broken JSON".as_slice()])
            .unwrap();
        assert!(SqliteStore::open(&db).unwrap().get(&key).is_err());
    }
    #[test]
    fn keys_are_canonical_and_reject_path_injection() {
        let a: serde_json::Value = serde_json::from_str(r#"{"blob":"a","config":"x"}"#).unwrap();
        let b: serde_json::Value = serde_json::from_str(r#"{"config":"x","blob":"a"}"#).unwrap();
        assert_eq!(DiffKey::new(&a).unwrap(), DiffKey::new(&b).unwrap());
        assert_ne!(
            DiffKey::new(&a).unwrap(),
            DiffKey::new(&serde_json::json!({"blob":"a","config":"y"})).unwrap()
        );
        assert!(serde_json::from_str::<DiffKey>(r#""../../outside""#).is_err());
    }
    #[test]
    fn concurrent_file_writers_publish_whole_records() {
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(FileStore::open(directory.path()).unwrap());
        let key = DiffKey::new(&"shared").unwrap();
        store.put(&key, &sample("initial")).unwrap();
        std::thread::scope(|scope| {
            for index in 0..4 {
                let store = &store;
                let key = &key;
                scope.spawn(move || {
                    for _ in 0..10 {
                        store.put(key, &sample(&index.to_string())).unwrap();
                        assert!(store.get(key).unwrap().is_some());
                    }
                });
            }
        });
    }
}
