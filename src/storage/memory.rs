use crate::search::store::{DiffKey, Store, StoredDiff};
use anyhow::Result;
use std::{collections::HashMap, sync::Mutex};

#[derive(Default)]
pub struct MemoryStore {
    entries: Mutex<HashMap<DiffKey, StoredDiff>>,
}
impl Store for MemoryStore {
    fn get(&self, key: &DiffKey) -> Result<Option<StoredDiff>> {
        Ok(self
            .entries
            .lock()
            .map_err(|_| anyhow::anyhow!("diff store lock poisoned"))?
            .get(key)
            .cloned())
    }
    fn put(&self, key: &DiffKey, diff: &StoredDiff) -> Result<()> {
        self.entries
            .lock()
            .map_err(|_| anyhow::anyhow!("diff store lock poisoned"))?
            .insert(key.clone(), diff.clone());
        Ok(())
    }
    fn remove(&self, key: &DiffKey) -> Result<()> {
        self.entries
            .lock()
            .map_err(|_| anyhow::anyhow!("diff store lock poisoned"))?
            .remove(key);
        Ok(())
    }
}
