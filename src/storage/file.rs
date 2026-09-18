use crate::search::store::{DiffKey, Store, StoredDiff};
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::PathBuf};

pub struct FileStore {
    directory: PathBuf,
}
impl FileStore {
    pub fn open(directory: impl Into<PathBuf>) -> Result<Self> {
        let directory = directory.into();
        fs::create_dir_all(&directory).context("create diff store directory")?;
        Ok(Self { directory })
    }
    fn path(&self, key: &DiffKey) -> PathBuf {
        self.directory.join(format!("{}.json", key.as_str()))
    }
}
#[derive(Serialize, Deserialize)]
struct Record {
    key: DiffKey,
    diff: StoredDiff,
}
impl Store for FileStore {
    fn get(&self, key: &DiffKey) -> Result<Option<StoredDiff>> {
        let bytes = match fs::read(self.path(key)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error).context("read diff store entry"),
        };
        let record: Record = serde_json::from_slice(&bytes).context("decode diff store entry")?;
        ensure!(&record.key == key, "diff store key mismatch");
        Ok(Some(record.diff))
    }
    fn put(&self, key: &DiffKey, diff: &StoredDiff) -> Result<()> {
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
    fn remove(&self, key: &DiffKey) -> Result<()> {
        match fs::remove_file(self.path(key)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}
