//! The native registry: the bundled plugins' code, compiled into diffr. Each
//! is an SDK [`Plugin`], the same trait a component's source implements, made
//! the same way from the same options string, and diffr calls it with the
//! same records it hands a component.
use super::host::{self, Host};
use super::Runner;
use anyhow::anyhow;
use diffr_plugin_sdk::types::{FileEntry, Move, Source};
use diffr_plugin_sdk::Plugin;
use std::rc::Rc;

/// Makes a native plugin's instance from its options string.
pub(crate) type Constructor = fn(Host, &str) -> anyhow::Result<Box<dyn Runner>>;

/// Every plugin diffr has native code for, by the name its `plugin.toml`
/// gives it.
pub(crate) const REGISTRY: &[(&str, Constructor)] = &[
    ("context", native::<diffr_plugin_context::Context>),
    ("hide-files", native::<diffr_plugin_hide_files::HideFiles>),
    (
        "deleted-bodies",
        native::<diffr_plugin_deleted_bodies::DeletedBodies>,
    ),
    (
        "test-bodies",
        native::<diffr_plugin_test_bodies::TestBodies>,
    ),
    (
        "removed-runs",
        native::<diffr_plugin_removed_runs::RemovedRuns>,
    ),
    ("group", native::<diffr_plugin_group::Group>),
];

/// The constructor of the native plugin `name`, if diffr has one.
pub(crate) fn lookup(name: &str) -> Option<Constructor> {
    REGISTRY
        .iter()
        .find(|(native, _)| *native == name)
        .map(|(_, constructor)| *constructor)
}

/// Make the native plugin `P`: deserialize `options` into `P::Options`, as a
/// component's `new` does, and call `P::new` with `host` behind the host
/// functions.
pub(crate) fn native<P: Plugin + Send + Sync + 'static>(
    host: Host,
    options: &str,
) -> anyhow::Result<Box<dyn Runner>> {
    let options: P::Options =
        serde_json::from_str(options).map_err(|error| anyhow!("invalid options: {error}"))?;
    let plugin = call(host, || P::new(options))?;
    Ok(Box::new(Native(plugin)))
}

/// Run `call`, one call of a native plugin, inside a host scope of its own.
fn call<R>(host: Host, call: impl FnOnce() -> anyhow::Result<R>) -> anyhow::Result<R> {
    let native = Rc::new(host::Native::new(host));
    let result = diffr_plugin_sdk::host::scope(native.clone(), call);
    native.finish()?;
    result
}

/// A native plugin's instance.
struct Native<P>(P);

impl<P: Plugin + Send + Sync> Runner for Native<P> {
    fn classify(&self, host: Host, file: &FileEntry) -> anyhow::Result<Vec<String>> {
        call(host, || self.0.classify(file))
    }

    fn mutate(
        &self,
        host: Host,
        file: &FileEntry,
        lhs: Option<&Source>,
        rhs: Option<&Source>,
    ) -> anyhow::Result<Vec<Move>> {
        call(host, || self.0.mutate(file, lhs, rhs))
    }
}
