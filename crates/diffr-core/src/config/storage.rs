//! Where search keeps computed diffs.
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct StoreConfig {
    /// Directory for computed diffs, relative to the source repository unless absolute.
    #[schemars(title = "Diff storage directory", extend("x-group" = "Storage"))]
    pub path: PathBuf,
}
impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            path: ".cache/diffr".into(),
        }
    }
}
