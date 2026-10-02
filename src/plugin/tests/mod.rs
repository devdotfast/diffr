//! Plugins run the way only the CLI runs them: bundled component plugins in
//! Wasmtime, and plugins inside the stream.
mod deferred;
mod summarize;

use super::wasm::Wasmtime;
use super::*;
use crate::config::Config;
use crate::pairing::Pairing;
use crate::params::DiffOptions;
use crate::protocol::{self, FileChange};
pub use diffr_core::plugin::testing::*;
use diffr_plugin_sdk::tree::{has_tag, is_fold, walk};
use diffr_plugin_sdk::{tree, types};
use host::Host;
use serde_json::json;

/// A pipeline of the bundled plugin `name` alone, made with its defaults and
/// `overrides`, a component run in Wasmtime.
pub fn bundled(name: &str, overrides: serde_json::Value) -> Pipeline {
    bundled_with(name, overrides, &Wasmtime::default())
}

#[test]
fn external_plugins_never_fall_back_to_a_native_registration() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("plugin.toml"),
        "name = 'context'\ntitle = 'External context'\n",
    )
    .unwrap();
    let config = Config::from_toml_in(
        "[plugins]\norder = ['external.context']\n[plugins.external.context]\npath = '.'\n",
        dir.path(),
    )
    .unwrap();
    let components = Wasmtime::default();
    let error = Pipeline::from_config(
        &config.plugins,
        system::environment(dir.path(), &components),
    )
    .err()
    .unwrap();
    let error = format!("{error:#}");
    assert!(error.contains("plugin.wasm"), "{error}");
    assert!(error.contains("plugins.external.context"), "{error}");
}
