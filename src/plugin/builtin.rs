//! The bundled plugins' folders, embedded. Each is a folder under
//! `plugins/` shaped like any plugin's: a `plugin.toml` (name, title, options
//! schema), its query files, and its code, a crate that uses the
//! plugin SDK. The code is compiled in and reached through the native
//! registry ([`super::native`]); `plugin.toml` and the query files are
//! embedded here, so `builtin:<plugin>/<path>` names `plugins/<plugin>/<path>`.
//! `plugins/shared/` is not a plugin: it holds the query files every bundled
//! plugin imports, as `builtin:shared/queries/<language>.scm`, and the
//! docstrings the plugins that collapse function bodies import, as
//! `builtin:shared/queries/<language>-docstrings.scm`.
use super::config::Manifest;
use std::sync::OnceLock;

/// The bundled plugins, in their default order.
pub(crate) const NAMES: [&str; 7] = [
    "context",
    "hide-files",
    "deleted-bodies",
    "test-bodies",
    "removed-runs",
    "summarize",
    "group",
];

macro_rules! embed {
    ($($path:literal),* $(,)?) => {
        &[$(($path, include_str!(concat!("../../plugins/", $path)))),*]
    };
}

/// Every embedded file, by its path under `plugins/`.
const FILES: &[(&str, &str)] = embed![
    "shared/queries/go.scm",
    "shared/queries/go-docstrings.scm",
    "shared/queries/javascript.scm",
    "shared/queries/javascript-docstrings.scm",
    "shared/queries/python.scm",
    "shared/queries/python-docstrings.scm",
    "shared/queries/rust.scm",
    "shared/queries/rust-docstrings.scm",
    "context/plugin.toml",
    "context/queries/go.scm",
    "context/queries/javascript.scm",
    "context/queries/python.scm",
    "context/queries/rust.scm",
    "hide-files/plugin.toml",
    "deleted-bodies/plugin.toml",
    "deleted-bodies/queries/go.scm",
    "deleted-bodies/queries/javascript.scm",
    "deleted-bodies/queries/python.scm",
    "deleted-bodies/queries/rust.scm",
    "test-bodies/plugin.toml",
    "test-bodies/queries/go.scm",
    "test-bodies/queries/javascript.scm",
    "test-bodies/queries/python.scm",
    "test-bodies/queries/rust.scm",
    "removed-runs/plugin.toml",
    "removed-runs/queries/go.scm",
    "removed-runs/queries/javascript.scm",
    "removed-runs/queries/python.scm",
    "removed-runs/queries/rust.scm",
    "summarize/plugin.toml",
    "summarize/queries/go.scm",
    "summarize/queries/javascript.scm",
    "summarize/queries/python.scm",
    "summarize/queries/rust.scm",
    "group/plugin.toml",
];

/// An embedded file by its normalized path under `plugins/`.
pub(crate) fn file(path: &str) -> Option<&'static str> {
    FILES
        .iter()
        .find(|(name, _)| *name == path)
        .map(|(_, text)| *text)
}

/// Every bundled plugin's `plugin.toml`, in [`NAMES`] order.
pub(crate) fn manifests() -> &'static [Manifest] {
    static MANIFESTS: OnceLock<Vec<Manifest>> = OnceLock::new();
    MANIFESTS.get_or_init(|| {
        NAMES
            .iter()
            .map(|name| {
                let path = format!("{name}/plugin.toml");
                let text = file(&path).unwrap_or_else(|| panic!("{path} is embedded"));
                Manifest::parse(text).unwrap_or_else(|error| panic!("plugins/{path}: {error}"))
            })
            .collect()
    })
}

pub(crate) fn manifest(name: &str) -> Option<&'static Manifest> {
    manifests().iter().find(|manifest| manifest.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::path::Path;

    #[test]
    fn every_plugin_folder_is_embedded_and_described() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins");
        let mut on_disk = BTreeSet::new();
        let mut folders = BTreeSet::new();
        for folder in std::fs::read_dir(&root).unwrap() {
            let folder = folder.unwrap().path();
            let name = folder.file_name().unwrap().to_str().unwrap().to_owned();
            if folder.join("plugin.toml").exists() {
                on_disk.insert(format!("{name}/plugin.toml"));
                folders.insert(name.clone());
            }
            let queries = folder.join("queries");
            if queries.exists() {
                for query in std::fs::read_dir(queries).unwrap() {
                    let query = query.unwrap().path();
                    let file = query.file_name().unwrap().to_str().unwrap();
                    on_disk.insert(format!("{name}/queries/{file}"));
                }
            }
        }
        let embedded: BTreeSet<String> = FILES.iter().map(|(path, _)| (*path).to_owned()).collect();
        assert_eq!(embedded, on_disk);
        assert_eq!(
            folders,
            NAMES.iter().map(|name| (*name).to_owned()).collect()
        );
        for name in NAMES {
            assert_eq!(manifest(name).unwrap().name, name);
            assert!(
                crate::plugin::native::lookup(name).unwrap().is_some(),
                "{name} has native code"
            );
        }
    }
}
