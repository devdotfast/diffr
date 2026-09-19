//! Storage contract for query-independent indexed diffs.
use crate::{
    pairing::Pairing,
    protocol::{FileChange, Source},
};
use anyhow::{ensure, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Bump when diff algorithms, parsers, or serialized tree semantics change.
const CACHE_VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), ":search-diff-v1");

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct DiffKey(String);
impl DiffKey {
    /// Hash a canonical JSON description of analysis settings and source identities.
    pub fn new(identity: &impl Serialize) -> Result<Self> {
        let mut canonical = serde_json::to_value((CACHE_VERSION, identity))?;
        canonical.sort_all_objects();
        Ok(Self(format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&canonical)?)
        )))
    }
    pub fn as_str(&self) -> &str {
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

/// Serializable computed data. Plugin instances and search highlights are not stored.
/// Fields remain internal; custom stores can serialize/deserialize this value with serde.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StoredDiff {
    pub(crate) entry: FileChange,
    pub(crate) sources: Pairing<Source>,
}

pub trait Store: Send + Sync {
    fn get(&self, key: &DiffKey) -> Result<Option<StoredDiff>>;
    fn put(&self, key: &DiffKey, diff: &StoredDiff) -> Result<()>;
    fn remove(&self, key: &DiffKey) -> Result<()>;
}
