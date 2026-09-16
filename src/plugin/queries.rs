//! Load the query files enabled plugins own and assemble one fold query per
//! language.
//!
//! Each plugin lists one query file per language key in its `plugin.toml`. A
//! path there is relative to the plugin's folder, a `builtin:<plugin>/<path>`
//! path naming a file of a bundled plugin embedded in diffr from `plugins/`,
//! or an absolute path. A query file may start with
//! `; inherits: a.scm, b.scm`: those files, relative to the importing file
//! (or `builtin:` paths), come first. Queries every bundled plugin shares
//! live in `plugins/shared/queries/<language>.scm`, imported as
//! `builtin:shared/queries/<language>.scm`. Within one language every file is
//! included at most once, however many plugins import it, and an import cycle
//! is an error.
use super::builtin;
use super::config::Queries;
use crate::config::query::QuerySource;
use crate::config::ConfigError;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const BUILTIN_PREFIX: &str = "builtin:";

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Location {
    /// A normalized path under `builtin:`.
    Builtin(String),
    /// A canonical filesystem path.
    File(PathBuf),
}

impl Location {
    fn name(&self) -> String {
        match self {
            Self::Builtin(path) => format!("{BUILTIN_PREFIX}{path}"),
            Self::File(path) => path.display().to_string(),
        }
    }

    fn builtin(path: &str) -> Result<Self, ConfigError> {
        let mut segments: Vec<&str> = Vec::new();
        for segment in path.split('/') {
            match segment {
                "" | "." => {}
                ".." => {
                    if segments.pop().is_none() {
                        return Err(ConfigError(format!(
                            "{BUILTIN_PREFIX}{path} leaves the bundled plugins"
                        )));
                    }
                }
                segment => segments.push(segment),
            }
        }
        let normalized = segments.join("/");
        if builtin::file(&normalized).is_none() {
            return Err(ConfigError(format!(
                "no bundled file {BUILTIN_PREFIX}{normalized}"
            )));
        }
        Ok(Self::Builtin(normalized))
    }

    fn file(path: &Path) -> Result<Self, ConfigError> {
        std::fs::canonicalize(path)
            .map(Self::File)
            .map_err(|error| ConfigError(format!("{}: {error}", path.display())))
    }

    /// Where `path`, written inside this file, points.
    fn resolve(&self, path: &str) -> Result<Self, ConfigError> {
        if let Some(builtin) = path.strip_prefix(BUILTIN_PREFIX) {
            return Self::builtin(builtin);
        }
        match self {
            Self::Builtin(own) => {
                let dir = own.rsplit_once('/').map_or("", |(dir, _)| dir);
                Self::builtin(&format!("{dir}/{path}"))
            }
            Self::File(own) => Self::file(
                &own.parent()
                    .expect("a canonical file path has a parent")
                    .join(path),
            ),
        }
    }

    fn read(&self) -> Result<String, ConfigError> {
        match self {
            Self::Builtin(path) => Ok(builtin::file(path)
                .expect("builtin locations are checked when resolved")
                .to_owned()),
            Self::File(path) => std::fs::read_to_string(path)
                .map_err(|error| ConfigError(format!("{}: {error}", path.display()))),
        }
    }
}

/// The paths a query file's first line imports.
fn imports(text: &str) -> Vec<&str> {
    text.lines()
        .next()
        .and_then(|line| line.trim().strip_prefix(";"))
        .and_then(|rest| rest.trim_start().strip_prefix("inherits:"))
        .map(|paths| {
            paths
                .split(',')
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Everything one language's query is assembled from so far.
#[derive(Default)]
struct Assembly {
    sources: Vec<QuerySource>,
    included: BTreeSet<Location>,
}

impl Assembly {
    fn include(
        &mut self,
        location: Location,
        stack: &mut Vec<Location>,
    ) -> Result<(), ConfigError> {
        if let Some(start) = stack.iter().position(|open| *open == location) {
            let cycle: Vec<String> = stack[start..]
                .iter()
                .chain([&location])
                .map(Location::name)
                .collect();
            return Err(ConfigError(format!(
                "query imports form a cycle: {}",
                cycle.join(" -> ")
            )));
        }
        if self.included.contains(&location) {
            return Ok(());
        }
        let text = location.read()?;
        stack.push(location.clone());
        for import in imports(&text) {
            let imported = location
                .resolve(import)
                .map_err(|error| ConfigError(format!("{}: {error}", location.name())))?;
            self.include(imported, stack)?;
        }
        stack.pop();
        self.sources.push(QuerySource {
            name: location.name(),
            text,
        });
        self.included.insert(location);
        Ok(())
    }
}

/// Per language key, the fold query sources of `plugins`, the enabled
/// plugins in `plugins.order` with their query files, imports before the
/// files that import them. Every path is absolute or `builtin:`, as
/// [`super::config::PluginsConfig::enabled_queries`] resolves them against
/// each plugin's folder.
pub(crate) fn assemble(
    plugins: &[(String, Queries)],
) -> Result<BTreeMap<String, Vec<QuerySource>>, ConfigError> {
    let mut languages: BTreeMap<String, Assembly> = BTreeMap::new();
    for (plugin, queries) in plugins {
        for (language, path) in queries {
            let key = format!("plugin {plugin}: queries.{language}");
            let location = match path.strip_prefix(BUILTIN_PREFIX) {
                Some(builtin) => Location::builtin(builtin),
                None if Path::new(path).is_absolute() => Location::file(Path::new(path)),
                None => Err(ConfigError(format!(
                    "relative path {path} must be absolute or builtin:"
                ))),
            }
            .map_err(|error| ConfigError(format!("{key}: {error}")))?;
            languages
                .entry(language.clone())
                .or_default()
                .include(location, &mut Vec::new())
                .map_err(|error| ConfigError(format!("{key}: {error}")))?;
        }
    }
    Ok(languages
        .into_iter()
        .map(|(language, assembly)| (language, assembly.sources))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn write(dir: &Path, name: &str, text: &str) -> String {
        let path = dir.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path.display().to_string()
    }

    fn names(sources: &[QuerySource]) -> Vec<&str> {
        sources.iter().map(|source| source.name.as_str()).collect()
    }

    #[test]
    fn every_bundled_query_resolves_and_compiles() {
        let assembled = assemble(&Config::default().plugins.enabled_queries()).unwrap();
        assert_eq!(
            assembled.keys().collect::<Vec<_>>(),
            [
                "go",
                "javascript",
                "javascriptjsx",
                "python",
                "rust",
                "typescript",
                "typescripttsx"
            ]
        );
        assert_eq!(
            names(&assembled["rust"]),
            [
                "builtin:shared/queries/rust.scm",
                "builtin:context/queries/rust.scm",
                "builtin:shared/queries/rust-docstrings.scm",
                "builtin:deleted-bodies/queries/rust.scm",
                "builtin:test-bodies/queries/rust.scm",
                "builtin:removed-runs/queries/rust.scm",
            ],
            "the shared imports are included once, before their first importer"
        );
        assert_eq!(
            names(&assembled["typescript"]),
            [
                "builtin:shared/queries/javascript.scm",
                "builtin:context/queries/javascript.scm",
                "builtin:shared/queries/javascript-docstrings.scm",
                "builtin:deleted-bodies/queries/javascript.scm",
                "builtin:test-bodies/queries/javascript.scm",
                "builtin:removed-runs/queries/javascript.scm",
            ],
        );
        Config::default().compile().unwrap();
    }

    #[test]
    fn disabled_plugins_contribute_no_queries() {
        let config = Config::from_toml(
            "[plugins.deleted-bodies]\nenabled = false\n[plugins.test-bodies]\nenabled = false\n",
        )
        .unwrap();
        let assembled = assemble(&config.plugins.enabled_queries()).unwrap();
        let names = names(&assembled["rust"]);
        assert!(!names.iter().any(|name| name.contains("deleted-bodies")));
        assert!(!names.iter().any(|name| name.contains("test-bodies")));
    }

    #[test]
    fn imports_resolve_against_their_files() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), "queries/shared.scm", "(block) @fold\n");
        let mine = write(
            dir.path(),
            "queries/rust/mine.scm",
            "; inherits: ../shared.scm, builtin:shared/queries/rust.scm\n((function_item body: (block) @fold) (#set! tag \"removed-runs:function\"))\n",
        );
        let plugins = [
            (
                "deleted-bodies".to_owned(),
                Queries::from([(
                    "rust".to_owned(),
                    "builtin:deleted-bodies/queries/rust.scm".to_owned(),
                )]),
            ),
            (
                "removed-runs".to_owned(),
                Queries::from([("rust".to_owned(), mine)]),
            ),
        ];
        let assembled = assemble(&plugins).unwrap();
        let names = names(&assembled["rust"]);
        let position = |suffix: &str| names.iter().position(|name| name.ends_with(suffix));
        assert!(
            position("queries/shared.scm").unwrap() < position("queries/rust/mine.scm").unwrap(),
            "{names:?}"
        );
        assert_eq!(
            names[..3],
            [
                "builtin:shared/queries/rust.scm",
                "builtin:shared/queries/rust-docstrings.scm",
                "builtin:deleted-bodies/queries/rust.scm"
            ],
            "imports come before their importer"
        );
        assert_eq!(
            names
                .iter()
                .filter(|name| **name == "builtin:shared/queries/rust.scm")
                .count(),
            1,
            "{names:?}"
        );
    }

    #[test]
    fn import_cycles_and_missing_files_are_errors() {
        let dir = tempfile::tempdir().unwrap();
        let a = write(dir.path(), "a.scm", "; inherits: b.scm\n");
        write(dir.path(), "b.scm", "; inherits: a.scm\n");
        let queries = |path: &str| {
            [(
                "removed-runs".to_owned(),
                Queries::from([("rust".to_owned(), path.to_owned())]),
            )]
        };
        let error = assemble(&queries(&a)).err().expect("a cycle").to_string();
        assert!(error.contains("cycle"), "{error}");
        assert!(
            error.starts_with("plugin removed-runs: queries.rust: "),
            "{error}"
        );
        let absent = dir.path().join("absent.scm").display().to_string();
        assert!(assemble(&queries(&absent)).is_err());
        let error = assemble(&queries("builtin:removed-runs/queries/absent.scm"))
            .err()
            .expect("unknown builtin")
            .to_string();
        assert!(
            error.contains("no bundled file builtin:removed-runs/queries/absent.scm"),
            "{error}"
        );
        let error = assemble(&queries("builtin:removed-runs/../../outside.scm"))
            .err()
            .expect("a path out of the bundled plugins")
            .to_string();
        assert!(error.contains("leaves the bundled plugins"), "{error}");
    }

    #[test]
    fn unbundled_plugin_queries_join_in_order_and_must_be_absolute_or_builtin() {
        let dir = tempfile::tempdir().unwrap();
        let absolute = write(
            dir.path(),
            "plugin/rust.scm",
            "; inherits: builtin:shared/queries/rust.scm\n((block) @fold (#set! tag \"mine:block\"))\n",
        );
        let mut plugins = Config::default().plugins.enabled_queries();
        let removed_runs = plugins
            .iter()
            .position(|(plugin, _)| plugin == "removed-runs")
            .unwrap();
        plugins.insert(
            removed_runs,
            (
                "mine".to_owned(),
                Queries::from([("rust".to_owned(), absolute)]),
            ),
        );
        let assembled = assemble(&plugins).unwrap();
        let names = names(&assembled["rust"]);
        let position = |suffix: &str| {
            names
                .iter()
                .position(|name| name.ends_with(suffix))
                .unwrap()
        };
        assert!(
            position("test-bodies/queries/rust.scm") < position("plugin/rust.scm"),
            "{names:?}"
        );
        assert!(
            position("plugin/rust.scm") < position("removed-runs/queries/rust.scm"),
            "{names:?}"
        );

        let relative = [(
            "mine".to_owned(),
            Queries::from([("rust".to_owned(), "plugin/rust.scm".to_owned())]),
        )];
        let error = assemble(&relative)
            .err()
            .expect("a relative plugin query")
            .to_string();
        assert_eq!(
            error,
            "plugin mine: queries.rust: relative path plugin/rust.scm must be absolute or builtin:"
        );
    }
}
