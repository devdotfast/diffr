//! `[plugins]`: the order plugins run in, and one entry per plugin with its
//! switch and options. Also `plugin.toml`, the static description every
//! plugin folder carries: the plugin's name, title, options schema and query
//! files.
//!
//! Every bundled plugin has an entry, pre-filled with the defaults its
//! `plugin.toml` declares; a file only writes the keys it changes. `order`
//! must name every entry exactly once. In an entry, `enabled` and `path` are
//! diffr's; every other key is one of the plugin's options, validated
//! against the schema in its `plugin.toml`.
//!
//! Every entry has a plugin folder: `plugin.toml`, its query files, and for
//! a component `plugin.wasm`. A bundled plugin's folder is embedded in diffr
//! ([`builtin`]); an entry with `path` names a folder on disk, relative to
//! the configuration file's directory, whose `plugin.toml` names the entry.
//! A bundled plugin's entry may set `path` too, and that folder then runs in
//! its place. Both kinds of folder are read the same way, and
//! [`super::Pipeline`] loads what they hold the same way.
use super::builtin;
use crate::config::ConfigError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Query files per language key.
pub(crate) type Queries = BTreeMap<String, String>;

/// The key in a plugin entry that turns the plugin on and off.
pub(crate) const ENABLED: &str = "enabled";

/// The key in a plugin entry that points at a plugin folder on disk.
pub(crate) const PATH: &str = "path";

/// The keys in a plugin entry that diffr owns: a plugin's options may not
/// use them.
pub(crate) const RESERVED: [&str; 2] = [ENABLED, PATH];

/// A plugin folder's description.
pub(crate) const MANIFEST_FILE: &str = "plugin.toml";

/// A plugin's `plugin.toml`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    /// The plugin's entry name in `[plugins]`, and the prefix of every tag
    /// its queries set: `<name>:<tag>`.
    pub(crate) name: String,
    /// The human name settings screens group the plugin's settings under.
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) description: String,
    /// How settings screens show the `enabled` switch, and whether the
    /// plugin is on by default. Without it the switch is titled "Run
    /// <title>" and the plugin is on.
    #[serde(default)]
    pub(crate) enabled: Option<Switch>,
    /// Each option's JSON Schema, in the order settings screens list them.
    /// Every option has a `title`; one with a `default` is pre-filled.
    #[serde(default)]
    pub(crate) options: Map<String, Value>,
    /// Query files per language key, relative to the plugin's folder.
    #[serde(default)]
    pub(crate) queries: Queries,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Switch {
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) description: String,
    /// Whether an entry that does not set `enabled` runs the plugin.
    #[serde(default = "on")]
    pub(crate) default: bool,
}

fn on() -> bool {
    true
}

impl Manifest {
    /// Parse and check a `plugin.toml`.
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let manifest: Self = toml::from_str(text).map_err(|error| error.to_string())?;
        manifest.check()?;
        Ok(manifest)
    }

    /// The manifest has a name and a title, every option has a title and
    /// is not a key diffr owns, the options schema is valid, and every
    /// default satisfies it.
    fn check(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("the plugin has no name".to_owned());
        }
        if self.title.trim().is_empty() {
            return Err(format!("{}: the plugin has no title", self.name));
        }
        if let Some(reserved) = RESERVED.iter().find(|key| self.options.contains_key(**key)) {
            return Err(format!(
                "{}: the options declare {reserved:?}, which diffr owns",
                self.name
            ));
        }
        if let Some((key, _)) = self.options.iter().find(|(_, option)| {
            option
                .get("title")
                .and_then(Value::as_str)
                .is_none_or(|title| title.trim().is_empty())
        }) {
            return Err(format!(
                "{}: option {key:?} has no title for settings screens",
                self.name
            ));
        }
        self.validate(&self.defaults())
            .map_err(|error| format!("{}: the defaults: {error}", self.name))
    }

    /// The JSON Schema of the plugin's options as an object: unknown keys
    /// are errors.
    fn options_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": self.options,
            "additionalProperties": false,
        })
    }

    /// Check `options` against the options schema. The message leads with
    /// the dotted path of the key it concerns, when there is one.
    pub(crate) fn validate(&self, options: &Map<String, Value>) -> Result<(), String> {
        let validator = jsonschema::validator_for(&self.options_schema())
            .map_err(|error| format!("the options schema is invalid: {error}"))?;
        let instance = Value::Object(options.clone());
        let errors: Vec<String> = validator
            .iter_errors(&instance)
            .map(|error| match error.instance_path().as_str() {
                "" => error.to_string(),
                path => format!(
                    "{}: {error}",
                    path.trim_start_matches('/').replace('/', ".")
                ),
            })
            .collect();
        match errors.is_empty() {
            true => Ok(()),
            false => Err(errors.join("; ")),
        }
    }

    /// Whether an entry that does not set `enabled` runs the plugin.
    pub(crate) fn enabled_by_default(&self) -> bool {
        self.enabled.as_ref().is_none_or(|switch| switch.default)
    }

    /// Every option that declares a default, with it.
    pub(crate) fn defaults(&self) -> Map<String, Value> {
        self.options
            .iter()
            .filter_map(|(key, option)| {
                option
                    .get("default")
                    .map(|default| (key.clone(), default.clone()))
            })
            .collect()
    }

    /// The entry this plugin adds to `diffr config schema` under `plugins`.
    /// Each option keeps its own schema, with the plugin's title as its
    /// `x-group` unless it sets one. An option whose type is an array or an
    /// object is marked `"x-settings": false`: settings screens edit
    /// scalars.
    pub(crate) fn settings_schema(&self) -> Value {
        let group = &self.title;
        let (title, description) = match &self.enabled {
            Some(switch) => (switch.title.clone(), switch.description.clone()),
            None => (format!("Run {group}"), self.description.clone()),
        };
        let enabled = self.enabled_by_default();
        let mut properties = Map::new();
        properties.insert(
            ENABLED.to_owned(),
            json!({
                "type": "boolean",
                "title": title,
                "description": description,
                "default": enabled,
                "x-group": group,
            }),
        );
        for (key, option) in &self.options {
            let mut option = option.clone();
            if let Some(option) = option.as_object_mut() {
                option
                    .entry("x-group")
                    .or_insert_with(|| Value::String(group.clone()));
                let structured = |kind: &Value| matches!(kind.as_str(), Some("array" | "object"));
                let structured = match option.get("type") {
                    Some(Value::Array(kinds)) => kinds.iter().any(structured),
                    Some(kind) => structured(kind),
                    None => false,
                };
                if structured {
                    option.insert("x-settings".to_owned(), Value::Bool(false));
                }
            }
            properties.insert(key.clone(), option);
        }
        json!({
            "type": "object",
            "title": group,
            "description": self.description,
            "properties": properties,
        })
    }
}

/// `[plugins]`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct PluginsConfig {
    /// The plugins in the order they run; each sees the region trees the
    /// ones before it left. Every entry is listed exactly once.
    pub(crate) order: Vec<String>,
    /// Every entry, by plugin name.
    #[serde(flatten)]
    pub(crate) entries: BTreeMap<String, Entry>,
}

/// One plugin's entry: diffr's keys, and the plugin's options.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct Entry {
    /// Whether the plugin runs; once resolved, set, from the file or the
    /// plugin's default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) enabled: Option<bool>,
    /// The plugin's folder on disk, as written: relative to the
    /// configuration file's directory, or absolute.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) path: Option<PathBuf>,
    /// The folder, loaded when the configuration resolves: `path`'s, or the
    /// bundled plugin's.
    #[serde(skip)]
    pub(crate) folder: Option<Folder>,
    #[serde(flatten)]
    pub(crate) options: Map<String, Value>,
}

/// Where a plugin folder is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Location {
    /// Embedded in diffr: `plugins/<name>/`.
    Bundled,
    /// On disk. Canonical.
    Disk(PathBuf),
}

/// A plugin folder and the `plugin.toml` in it.
#[derive(Clone, Debug)]
pub(crate) struct Folder {
    pub(crate) location: Location,
    pub(crate) manifest: Manifest,
}

impl Folder {
    /// The bundled plugin `name`'s embedded folder.
    fn bundled(name: &str) -> Option<Self> {
        builtin::manifest(name).map(|manifest| Self {
            location: Location::Bundled,
            manifest: manifest.clone(),
        })
    }

    /// The folder's query files per language key, each path resolved against
    /// the folder: `builtin:<name>/<path>` for an embedded folder, absolute
    /// for one on disk. A `builtin:` path stays as written.
    pub(crate) fn queries(&self) -> Queries {
        self.manifest
            .queries
            .iter()
            .map(|(language, path)| {
                let path = match (path.starts_with("builtin:"), &self.location) {
                    (true, _) => path.clone(),
                    (false, Location::Bundled) => format!("builtin:{}/{path}", self.manifest.name),
                    (false, Location::Disk(dir)) => dir.join(path).display().to_string(),
                };
                (language.clone(), path)
            })
            .collect()
    }

    /// Load the folder at `dir` for the entry `name`: its `plugin.toml` must
    /// name the entry.
    fn load(name: &str, dir: &Path) -> Result<Self, ConfigError> {
        let dir = std::fs::canonicalize(dir)
            .map_err(|error| ConfigError(format!("{}: {error}", dir.display())))?;
        let manifest_path = dir.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&manifest_path)
            .map_err(|error| ConfigError(format!("{}: {error}", manifest_path.display())))?;
        let manifest = Manifest::parse(&text)
            .map_err(|error| ConfigError(format!("{}: {error}", manifest_path.display())))?;
        if manifest.name != name {
            return Err(ConfigError(format!(
                "{}: the plugin is named {:?}, not {name:?}",
                manifest_path.display(),
                manifest.name
            )));
        }
        Ok(Self {
            location: Location::Disk(dir),
            manifest,
        })
    }
}

impl Entry {
    /// The entry's folder. Every entry has one once the configuration
    /// resolves.
    pub(crate) fn folder(&self) -> &Folder {
        self.folder
            .as_ref()
            .expect("a resolved entry has its plugin folder")
    }

    /// Whether the plugin runs.
    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled.expect("a resolved entry is enabled or not")
    }
}

impl Default for PluginsConfig {
    fn default() -> Self {
        let mut config = Self {
            order: builtin::NAMES.map(str::to_owned).to_vec(),
            entries: BTreeMap::new(),
        };
        config
            .resolve(Path::new(""))
            .expect("the bundled plugins' defaults are valid");
        config
    }
}

impl PluginsConfig {
    /// Load every entry's folder, the bundled plugin's or the one `path`
    /// names relative to `base`, check every plugin's options against its
    /// schema and fill in the defaults and `enabled`, add an entry for every
    /// bundled plugin the file did not write, and check that `order` names
    /// every entry exactly once.
    pub(crate) fn resolve(&mut self, base: &Path) -> Result<(), ConfigError> {
        for name in builtin::NAMES {
            self.entries.entry(name.to_owned()).or_default();
        }
        for (name, entry) in &mut self.entries {
            let folder = match &entry.path {
                Some(path) => Folder::load(name, &base.join(path))
                    .map_err(|error| ConfigError(format!("plugins.{name}: {error}")))?,
                None => Folder::bundled(name).ok_or_else(|| {
                    ConfigError(format!(
                        "plugins.{name}: no bundled plugin has this name, and the entry has no path"
                    ))
                })?,
            };
            let manifest = &folder.manifest;
            entry.enabled.get_or_insert(manifest.enabled_by_default());
            manifest
                .validate(&entry.options)
                .map_err(|error| ConfigError(format!("plugins.{name}: {error}")))?;
            for (key, default) in manifest.defaults() {
                entry.options.entry(key).or_insert(default);
            }
            entry.folder = Some(folder);
        }
        let mut seen = BTreeSet::new();
        for name in &self.order {
            if !self.entries.contains_key(name) {
                return Err(ConfigError(format!(
                    "plugins.order: no plugin entry named {name:?}"
                )));
            }
            if !seen.insert(name.as_str()) {
                return Err(ConfigError(format!(
                    "plugins.order: {name:?} is listed twice"
                )));
            }
        }
        if let Some(missing) = self
            .entries
            .keys()
            .find(|name| !seen.contains(name.as_str()))
        {
            return Err(ConfigError(format!(
                "plugins.order: the plugin entry {missing:?} is not listed"
            )));
        }
        Ok(())
    }

    /// The enabled entries, in `order`.
    pub(crate) fn enabled(&self) -> impl Iterator<Item = (&str, &Entry)> {
        self.order.iter().filter_map(|name| {
            self.entries
                .get(name)
                .filter(|entry| entry.is_enabled())
                .map(|entry| (name.as_str(), entry))
        })
    }

    /// The query files of every enabled plugin, in `order`, resolved
    /// against each plugin's folder.
    pub(crate) fn enabled_queries(&self) -> Vec<(String, Queries)> {
        self.enabled()
            .map(|(name, entry)| (name.to_owned(), entry.folder().queries()))
            .filter(|(_, queries)| !queries.is_empty())
            .collect()
    }

    /// The `plugins` property of `diffr config schema`: `order`, and every
    /// bundled plugin's entry.
    pub(crate) fn schema() -> Value {
        let mut properties = Map::new();
        properties.insert(
            "order".to_owned(),
            json!({
                "type": "array",
                "items": {"type": "string"},
                "description": "The plugins in the order they run; each sees the region trees the ones before it left. Every entry is listed exactly once.",
                "default": builtin::NAMES,
                "x-settings": false,
            }),
        );
        for manifest in builtin::manifests() {
            properties.insert(manifest.name.clone(), manifest.settings_schema());
        }
        json!({
            "type": "object",
            "description": "The plugins that decide what starts collapsed, hidden, linked or grouped, and the fold queries they own.",
            "properties": properties,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use serde_json::Value;

    /// Every setting the schema lists, as a settings screen flattens it:
    /// `(dotted key, title, group, type)`. A key marked `"x-settings": false`
    /// is not a setting.
    fn settings(schema: &Value) -> Vec<(String, String, String, String)> {
        fn resolve<'a>(node: &'a Value, root: &'a Value) -> &'a Value {
            match node.get("$ref").and_then(Value::as_str) {
                Some(reference) => {
                    let name = reference.rsplit('/').next().unwrap();
                    &root["$defs"][name]
                }
                None => node,
            }
        }
        fn walk(
            node: &Value,
            root: &Value,
            key: &str,
            out: &mut Vec<(String, String, String, String)>,
        ) {
            let resolved = resolve(node, root);
            if node.get("x-settings") == Some(&Value::Bool(false)) {
                return;
            }
            if let Some(properties) = resolved.get("properties").and_then(Value::as_object) {
                for (name, child) in properties {
                    let key = if key.is_empty() {
                        name.clone()
                    } else {
                        format!("{key}.{name}")
                    };
                    walk(child, root, &key, out);
                }
                return;
            }
            let text = |field: &str| {
                node.get(field)
                    .or_else(|| resolved.get(field))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            let kind = resolved
                .get("type")
                .map(|kind| kind.to_string())
                .or_else(|| resolved.get("enum").map(|_| "enum".to_owned()))
                .or_else(|| resolved.get("anyOf").map(|_| "optional".to_owned()))
                .expect("a typed setting");
            out.push((key.to_owned(), text("title"), text("x-group"), kind));
        }
        let mut out = Vec::new();
        walk(schema, schema, "", &mut out);
        out
    }

    #[test]
    fn every_plugin_setting_has_a_title_and_a_group_and_is_a_scalar() {
        let schema = Config::schema();
        let settings = settings(&schema);
        let plugins: Vec<_> = settings
            .iter()
            .filter(|(key, ..)| key.starts_with("plugins."))
            .collect();
        assert!(plugins.len() > 10, "{plugins:?}");
        for (key, title, group, kind) in &settings {
            assert!(
                !title.is_empty() && !group.is_empty(),
                "{key} has no title or group"
            );
            assert!(
                !kind.contains("array") && !kind.contains("object"),
                "{key} is {kind}"
            );
        }
        let group = |key: &str| {
            plugins
                .iter()
                .find(|(own, ..)| own == key)
                .map(|(_, title, group, _)| (title.as_str(), group.as_str()))
                .unwrap_or_else(|| panic!("no setting {key}"))
        };
        assert_eq!(
            group("plugins.deleted-bodies.enabled"),
            (
                "Collapse deleted function bodies",
                "Deleted function bodies"
            )
        );
        assert!(schema["properties"].get("languages").is_none());
        let plugins = &schema["properties"]["plugins"]["properties"];
        assert_eq!(plugins["order"]["x-settings"], false);
        assert_eq!(plugins["order"]["type"], "array");
        assert_eq!(
            plugins["hide-files"]["properties"]["tags"]["x-settings"],
            false
        );
        let keys: Vec<&String> = plugins.as_object().unwrap().keys().collect();
        assert_eq!(
            keys,
            [
                "order",
                "context",
                "hide-files",
                "deleted-bodies",
                "test-bodies",
                "removed-runs",
                "group"
            ]
        );
    }

    #[test]
    fn order_names_every_entry_exactly_once() {
        let order = |names: &str| Config::from_toml(&format!("[plugins]\norder = [{names}]"));
        assert!(order("'context', 'hide-files', 'deleted-bodies', 'test-bodies', 'removed-runs', 'group'").is_ok());
        let error = order(
            "'context', 'hide-files', 'deleted-bodies', 'test-bodies', 'removed-runs'",
        )
        .err()
        .unwrap()
        .to_string();
        assert!(error.contains("\"group\" is not listed"), "{error}");
        let error = order("'context', 'hide-files', 'deleted-bodies', 'test-bodies', 'removed-runs', 'group', 'mine'")
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("no plugin entry named \"mine\""), "{error}");
        let error = order("'context', 'hide-files', 'hide-files', 'deleted-bodies', 'test-bodies', 'removed-runs', 'group'")
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("listed twice"), "{error}");
        let error = Config::from_toml("[plugins.mine]\nenabled = true")
            .err()
            .unwrap()
            .to_string();
        assert_eq!(
            error,
            "plugins.mine: no bundled plugin has this name, and the entry has no path"
        );
    }

    #[test]
    fn a_path_entry_loads_its_folder_relative_to_the_config_directory() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("plugins/mine");
        std::fs::create_dir_all(folder.join("queries")).unwrap();
        std::fs::write(
            folder.join("plugin.toml"),
            "name = 'mine'\ntitle = 'Mine'\n[options.depth]\ntype = 'integer'\ntitle = 'Depth'\ndefault = 2\n[queries]\nrust = 'queries/rust.scm'\npython = 'builtin:shared/queries/python.scm'\n",
        )
        .unwrap();
        std::fs::write(
            folder.join("queries/rust.scm"),
            "((block) @fold (#set! tag \"mine:block\"))\n",
        )
        .unwrap();
        let order = "order = ['context', 'hide-files', 'deleted-bodies', 'test-bodies', 'removed-runs', 'group', 'mine']";
        let config = Config::from_toml_in(
            &format!("[plugins]\n{order}\n[plugins.mine]\npath = 'plugins/mine'\n"),
            dir.path(),
        )
        .unwrap();
        let entry = &config.plugins.entries["mine"];
        assert_eq!(entry.options["depth"], 2);
        assert!(entry.options.get("path").is_none());
        let queries = config.plugins.enabled_queries();
        let (_, mine) = queries.iter().find(|(name, _)| name == "mine").unwrap();
        let canonical = std::fs::canonicalize(&folder).unwrap();
        assert_eq!(
            mine["rust"],
            canonical.join("queries/rust.scm").display().to_string()
        );
        assert_eq!(mine["python"], "builtin:shared/queries/python.scm");
        config.compile().unwrap();

        let error = |toml: &str| {
            Config::from_toml_in(toml, dir.path())
                .err()
                .unwrap()
                .to_string()
        };
        let renamed = error(&format!(
            "[plugins]\n{}\n[plugins.other]\npath = 'plugins/mine'\n",
            order.replace("'mine'", "'other'")
        ));
        assert!(
            renamed.starts_with("plugins.other: ")
                && renamed.ends_with("the plugin is named \"mine\", not \"other\""),
            "{renamed}"
        );
        let missing = error("[plugins.group]\npath = 'plugins/absent'\n");
        assert!(missing.starts_with("plugins.group: "), "{missing}");
        std::fs::write(
            folder.join("plugin.toml"),
            "name = 'mine'\ntitle = 'Mine'\n[options.path]\ntype = 'string'\ntitle = 'Path'\n",
        )
        .unwrap();
        let reserved = error(&format!(
            "[plugins]\n{order}\n[plugins.mine]\npath = 'plugins/mine'\n"
        ));
        assert!(reserved.contains("which diffr owns"), "{reserved}");
    }

    #[test]
    fn options_are_checked_against_the_plugin_toml_and_filled_with_its_defaults() {
        let config = Config::from_toml(
            "[plugins.deleted-bodies]\nmin_lines = 30\n[plugins.hide-files]\nenabled = false\n",
        )
        .unwrap();
        let deleted = &config.plugins.entries["deleted-bodies"];
        assert_eq!(deleted.enabled, Some(true));
        assert_eq!(deleted.options["min_lines"], 30);
        let hide = &config.plugins.entries["hide-files"];
        assert_eq!(hide.enabled, Some(false));
        assert_eq!(hide.options["deleted"], true);
        assert_eq!(
            hide.options["tags"],
            serde_json::json!(["generated", "vendored", "test"])
        );
        let error = |toml: &str| Config::from_toml(toml).err().unwrap().to_string();
        let typo = error("[plugins.deleted-bodies]\ntypo = 1\n");
        assert!(
            typo.starts_with("plugins.deleted-bodies: ") && typo.contains("typo"),
            "{typo}"
        );
        let mistyped = error("[plugins.deleted-bodies]\nmin_lines = 'many'\n");
        assert!(
            mistyped.starts_with("plugins.deleted-bodies: min_lines: "),
            "{mistyped}"
        );
        let negative = error("[plugins.removed-runs]\nmin_lines = -1\n");
        assert!(
            negative.starts_with("plugins.removed-runs: min_lines: "),
            "{negative}"
        );
    }
}
