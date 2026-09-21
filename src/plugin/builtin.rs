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

// Manifests, queries and optional WASM components of plugin dependencies.
include!(concat!(env!("OUT_DIR"), "/bundled_assets.rs"));

pub(crate) fn component(name: &str) -> Option<&'static [u8]> {
    COMPONENTS
        .iter()
        .find(|(own, _)| *own == name)
        .map(|(_, bytes)| *bytes)
}

/// An embedded file by its normalized path under `plugins/`.
pub(crate) fn file(path: &str) -> Option<&'static str> {
    FILES
        .iter()
        .find(|(name, _)| *name == path)
        .map(|(_, text)| *text)
}

/// Every bundled plugin manifest, discovered from Cargo dependencies.
pub(crate) fn manifests() -> &'static [Manifest] {
    static MANIFESTS: OnceLock<Vec<Manifest>> = OnceLock::new();
    MANIFESTS.get_or_init(|| {
        FILES
            .iter()
            .filter(|(path, _)| path.ends_with("/plugin.toml"))
            .map(|(path, text)| {
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
            manifests()
                .iter()
                .map(|manifest| manifest.name.clone())
                .collect()
        );
        for manifest in manifests() {
            let name = manifest.name.as_str();
            assert_eq!(super::manifest(name).unwrap().name, name);
            assert!(
                crate::plugin::native::lookup(name).unwrap().is_some() || component(name).is_some(),
                "{name} has native code"
            );
        }
    }
}
