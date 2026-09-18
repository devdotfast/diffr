use crate::search::store::{DiffKey, Store, StoredDiff};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::{fs, path::Path, sync::Mutex};

pub struct SqliteStore {
    connection: Mutex<Connection>,
}
impl SqliteStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(path)?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             CREATE TABLE IF NOT EXISTS diffs (key TEXT PRIMARY KEY, data BLOB NOT NULL);",
        )?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
}
impl Store for SqliteStore {
    fn get(&self, key: &DiffKey) -> Result<Option<StoredDiff>> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("diff store lock poisoned"))?;
        let bytes: Option<Vec<u8>> = connection
            .query_row(
                "SELECT data FROM diffs WHERE key = ?1",
                [key.as_str()],
                |row| row.get(0),
            )
            .optional()?;
        bytes
            .map(|bytes| serde_json::from_slice(&bytes).context("decode SQLite diff entry"))
            .transpose()
    }
    fn put(&self, key: &DiffKey, diff: &StoredDiff) -> Result<()> {
        let connection = self
            .connection
            .lock()
            .map_err(|_| anyhow::anyhow!("diff store lock poisoned"))?;
        connection.execute(
            "INSERT INTO diffs(key,data) VALUES (?1,?2)
             ON CONFLICT(key) DO UPDATE SET data=excluded.data",
            params![key.as_str(), serde_json::to_vec(diff)?],
        )?;
        Ok(())
    }
    fn remove(&self, key: &DiffKey) -> Result<()> {
        self.connection
            .lock()
            .map_err(|_| anyhow::anyhow!("diff store lock poisoned"))?
            .execute("DELETE FROM diffs WHERE key = ?1", [key.as_str()])?;
        Ok(())
    }
}
