//! `diffr config`: read the schema and resolved values, and write one key to
//! the global file.
use super::{directory_of, Config, ConfigError};
use crate::plugin::config::PATH;
use serde_json::{json, Value};
use std::path::Path;

/// The resolved configuration as JSON, with the same nesting as the TOML.
/// The summarizer's API key is redacted unless `reveal` is set.
pub(crate) fn show(config: &Config, reveal: bool) -> serde_json::Value {
    serde_json::to_value(redacted(config, reveal)).expect("config serializes")
}

/// The configuration to print: the summarizer's API key replaced by
/// `<redacted>` unless `reveal` is set.
pub(crate) fn redacted(config: &Config, reveal: bool) -> Config {
    let mut shown = config.clone();
    if !reveal {
        for entry in shown.plugins.entries.values_mut() {
            if let Some(key) = entry
                .options
                .get_mut("api_key")
                .filter(|key| key.as_str().is_some_and(|key| !key.is_empty()))
            {
                *key = Value::String("<redacted>".into());
            }
        }
    }
    shown
}

/// Materialize resolved defaults on edit, then write `key = value` while
/// preserving existing values and comments. `value` is read as the type the schema gives the key (see
/// [`typed_value`]), then the whole file is validated, before anything
/// touches the disk: unknown keys, text that is not the key's type, and
/// values the configuration rejects are errors.
pub(crate) fn set(path: &Path, key: &str, value: &str) -> Result<(), ConfigError> {
    if key.is_empty() || key.split('.').any(str::is_empty) {
        return Err(ConfigError(format!("invalid key {key:?}")));
    }
    let existing = read(path)?;
    let directory = directory_of(path);
    let typed = typed_value(key, &setting_schema(key, &existing, directory)?, value)?;
    let mut document: toml_edit::DocumentMut = existing
        .parse()
        .map_err(|error| ConfigError(format!("{}: {error}", path.display())))?;
    materialize(&mut document, directory)?;
    assign(&mut document, key, &typed)?;
    write(path, document)
}

fn read(path: &Path) -> Result<String, ConfigError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(error) => Err(ConfigError(format!("{}: {error}", path.display()))),
    }
}

/// Fill missing fields from the resolved configuration, without replacing
/// existing values, comments, formatting, or explicit plugin membership.
fn materialize(document: &mut toml_edit::DocumentMut, directory: &Path) -> Result<(), ConfigError> {
    let resolved = Config::from_toml_in(&document.to_string(), directory)?;
    let values = toml::Value::try_from(resolved).map_err(|error| ConfigError(error.to_string()))?;
    fill_missing(
        document.as_table_mut(),
        values.as_table().expect("config is a table"),
    )
}

fn fill_missing(
    target: &mut dyn toml_edit::TableLike,
    values: &toml::Table,
) -> Result<(), ConfigError> {
    for (key, value) in values {
        if let toml::Value::Table(children) = value {
            if !target.contains_key(key) {
                target.insert(key, toml_edit::Item::Table(toml_edit::Table::new()));
            }
            let nested = target
                .get_mut(key)
                .and_then(toml_edit::Item::as_table_like_mut)
                .ok_or_else(|| ConfigError(format!("{key}: expected a table")))?;
            fill_missing(nested, children)?;
        } else if !target.contains_key(key) {
            target.insert(key, toml_edit::Item::Value(edit_value(value)?));
        }
    }
    Ok(())
}

fn write(path: &Path, mut document: toml_edit::DocumentMut) -> Result<(), ConfigError> {
    materialize(&mut document, directory_of(path))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| ConfigError(format!("{}: {error}", parent.display())))?;
    }
    std::fs::write(path, document.to_string())
        .map_err(|error| ConfigError(format!("{}: {error}", path.display())))
}

/// The JSON Schema of the setting at the dotted `key`, as `diffr config
/// schema` describes it. A plugin entry's keys come from the entry's own
/// `plugin.toml`, which for an entry with `path` is in the folder the file `existing`
/// (in `directory`) points it at; `path` is diffr's, and a string.
fn setting_schema(key: &str, existing: &str, directory: &Path) -> Result<Value, ConfigError> {
    let unknown = || ConfigError(format!("{key}: unknown key"));
    if let ["plugins", namespace, name, field] = key.split('.').collect::<Vec<_>>().as_slice() {
        if *field == PATH {
            return Ok(json!({"type": "string"}));
        }
        let config = Config::from_toml_in(existing, directory)?;
        let manifest = &config
            .plugins
            .entries
            .get(&format!("{namespace}.{name}"))
            .ok_or_else(unknown)?
            .folder()
            .manifest;
        return manifest.settings_schema()["properties"]
            .get(*field)
            .cloned()
            .ok_or_else(unknown);
    }
    let root = Config::schema();
    let resolve = |node: &Value| -> Value {
        match node.get("$ref").and_then(Value::as_str) {
            Some(reference) => {
                let name = reference.rsplit('/').next().expect("split yields a piece");
                root["$defs"][name].clone()
            }
            None => node.clone(),
        }
    };
    let mut node = root.clone();
    for segment in key.split('.') {
        node = resolve(
            resolve(&node)
                .get("properties")
                .and_then(|properties| properties.get(segment))
                .ok_or_else(unknown)?,
        );
    }
    if node.get("properties").is_some() {
        return Err(ConfigError(format!(
            "{key} is a table; set one of its keys"
        )));
    }
    Ok(node)
}

/// `text` read as the type `schema` gives the setting `key`, and nothing
/// else: a string setting takes the text as it is, an integer, number or
/// boolean setting takes only that TOML literal, an array setting a TOML
/// array, and an enum setting one of its choices.
fn typed_value(key: &str, schema: &Value, text: &str) -> Result<toml::Value, ConfigError> {
    let mistyped =
        |expected: &str| ConfigError(format!("{key}: expected {expected}, got {text:?}"));
    if let Some(choices) = schema.get("enum").and_then(Value::as_array) {
        if !choices.iter().any(|choice| choice.as_str() == Some(text)) {
            let choices: Vec<String> = choices.iter().map(Value::to_string).collect();
            return Err(mistyped(&format!("one of {}", choices.join(", "))));
        }
        return Ok(toml::Value::String(text.to_owned()));
    }
    // An optional setting is its type or null; `set` writes the type.
    let kinds: Vec<&str> = match schema.get("type") {
        Some(Value::String(kind)) => vec![kind.as_str()],
        Some(Value::Array(kinds)) => kinds
            .iter()
            .filter_map(Value::as_str)
            .filter(|kind| *kind != "null")
            .collect(),
        _ => Vec::new(),
    };
    let [kind] = kinds.as_slice() else {
        return Err(ConfigError(format!(
            "{key}: the schema gives it no single type, so config set cannot write it"
        )));
    };
    Ok(match *kind {
        "string" => toml::Value::String(text.to_owned()),
        "integer" => toml::Value::Integer(text.parse().map_err(|_| mistyped("an integer"))?),
        "number" => toml::Value::Float(text.parse().map_err(|_| mistyped("a number"))?),
        "boolean" => match text {
            "true" => toml::Value::Boolean(true),
            "false" => toml::Value::Boolean(false),
            _ => return Err(mistyped("true or false")),
        },
        "array" => {
            let mut table =
                toml::from_str::<toml::Table>(&format!("value = {text}")).map_err(|error| {
                    ConfigError(format!(
                        "{key}: expected a TOML array such as [\"a\", \"b\"], got {text:?}: {}",
                        error.message()
                    ))
                })?;
            match table
                .remove("value")
                .expect("the table has the one key parsed")
            {
                value @ toml::Value::Array(_) => value,
                _ => return Err(mistyped("a TOML array such as [\"a\", \"b\"]")),
            }
        }
        other => {
            return Err(ConfigError(format!(
                "{key}: config set cannot write a value of type {other}"
            )));
        }
    })
}

fn assign(
    document: &mut toml_edit::DocumentMut,
    key: &str,
    value: &toml::Value,
) -> Result<(), ConfigError> {
    let mut segments = key.split('.').peekable();
    let mut item = document.as_item_mut();
    while let Some(segment) = segments.next() {
        if segments.peek().is_none() {
            match item.as_table_like_mut() {
                Some(table) => table.insert(segment, toml_edit::Item::Value(edit_value(value)?)),
                None => return Err(ConfigError(format!("{key}: parent is not a table"))),
            };
            return Ok(());
        }
        let table = item
            .as_table_like_mut()
            .ok_or_else(|| ConfigError(format!("{key}: parent is not a table")))?;
        if !table.contains_key(segment) {
            let mut nested = toml_edit::Table::new();
            nested.set_implicit(true);
            table.insert(segment, toml_edit::Item::Table(nested));
        }
        item = table.get_mut(segment).expect("just inserted");
    }
    Err(ConfigError(format!("invalid key {key:?}")))
}

fn edit_value(value: &toml::Value) -> Result<toml_edit::Value, ConfigError> {
    Ok(match value {
        toml::Value::String(text) => toml_edit::Value::from(text.as_str()),
        toml::Value::Integer(number) => toml_edit::Value::from(*number),
        toml::Value::Float(number) => toml_edit::Value::from(*number),
        toml::Value::Boolean(flag) => toml_edit::Value::from(*flag),
        toml::Value::Array(items) => {
            let mut array = toml_edit::Array::new();
            for item in items {
                array.push(edit_value(item)?);
            }
            toml_edit::Value::Array(array)
        }
        toml::Value::Table(_) | toml::Value::Datetime(_) => {
            return Err(ConfigError(
                "config set takes a scalar or array value".into(),
            ));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn set_writes_typed_values_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        set(&path, "diff.graph_limit", "20").unwrap();
        // `theme.name` is a string, so digits are its text.
        set(&path, "theme.name", "1234").unwrap();
        set(&path, "theme.path", "themes/mine.toml").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("graph_limit = 20"), "{text}");
        assert!(text.contains("name = \"1234\""), "{text}");
        assert!(text.contains("path = \"themes/mine.toml\""), "{text}");
        let config = Config::from_toml(&text).unwrap();
        assert_eq!(config.diff.graph_limit, 20);
        assert_eq!(config.theme.name, "1234");
    }

    #[test]
    fn plugin_keys_are_written_under_their_quoted_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        set(&path, "plugins.bundled.deleted-bodies.min_lines", "30").unwrap();
        set(&path, "plugins.bundled.summarize.api_key", "secret").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let config = Config::from_toml(&text).unwrap();
        assert_eq!(
            config.plugins.entries["bundled.deleted-bodies"].options["min_lines"],
            30
        );
        assert!(set(&path, "plugins.bundled.deleted-bodies.typo", "1").is_err());
        assert!(set(&path, "plugins.bundled.deleted-bodies.min_lines", "-1").is_err());
        assert!(set(&path, "plugins.order", "[\"group\"]").is_err());
        let error = |key: &str, value: &str| set(&path, key, value).unwrap_err().to_string();
        assert_eq!(
            error("plugins.bundled.context.enabled", "yes"),
            "plugins.bundled.context.enabled: expected true or false, got \"yes\""
        );
        assert!(error("plugins.order", "context")
            .starts_with("plugins.order: expected a TOML array such as [\"a\", \"b\"]"));
        assert_eq!(
            error("plugins.bundled.context.lines", "many"),
            "plugins.bundled.context.lines: expected an integer, got \"many\""
        );
        assert_eq!(
            error("plugins.bundled.summarize.provider", "openai"),
            "plugins.bundled.summarize.provider: expected one of \"gemini\", got \"openai\""
        );
        assert_eq!(
            error("plugins.external.mine.enabled", "true"),
            "plugins.external.mine.enabled: unknown key"
        );
        set(&path, "plugins.bundled.context.enabled", "false").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            Config::from_toml(&text).unwrap().plugins.entries["bundled.context"].enabled,
            Some(false)
        );
        let shown = show(&config, false);
        assert_eq!(
            shown["plugins"]["bundled"]["summarize"]["api_key"],
            "<redacted>"
        );
        assert_eq!(
            show(&config, true)["plugins"]["bundled"]["summarize"]["api_key"],
            "secret"
        );
        assert_eq!(
            shown["plugins"]["bundled"]["deleted-bodies"],
            serde_json::json!({"enabled": true, "min_lines": 30})
        );
        assert!(shown["plugins"]["bundled"]["summarize"]["system_prompt"]
            .as_str()
            .unwrap()
            .starts_with("For each listed fold"));
        let text = toml::to_string_pretty(&redacted(&config, false)).unwrap();
        assert!(text.contains("[plugins.bundled.summarize]"), "{text}");
        assert!(text.contains("system_prompt = "), "{text}");
    }

    #[test]
    fn a_wasm_plugin_option_takes_the_type_its_folder_declares() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("plugins/mine");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("plugin.toml"),
            "name = 'mine'\ntitle = 'Mine'\n[options.depth]\ntype = 'integer'\ntitle = 'Depth'\ndefault = 2\n",
        )
        .unwrap();
        let path = dir.path().join("config.toml");
        let order = "order = ['bundled.context', 'bundled.hide-files', 'bundled.deleted-bodies', 'bundled.test-bodies', 'bundled.removed-runs', 'bundled.summarize', 'bundled.group', 'external.mine']";
        std::fs::write(
            &path,
            format!("[plugins]\n{order}\n[plugins.external.mine]\npath = 'plugins/mine'\n"),
        )
        .unwrap();
        set(&path, "plugins.external.mine.depth", "3").unwrap();
        set(&path, "plugins.external.mine.enabled", "false").unwrap();
        set(&path, "plugins.external.mine.path", "plugins/mine").unwrap();
        let error = |key: &str, value: &str| set(&path, key, value).unwrap_err().to_string();
        assert_eq!(
            error("plugins.external.mine.depth", "deep"),
            "plugins.external.mine.depth: expected an integer, got \"deep\""
        );
        assert_eq!(
            error("plugins.external.mine.typo", "1"),
            "plugins.external.mine.typo: unknown key"
        );
        let text = std::fs::read_to_string(&path).unwrap();
        let config = Config::from_toml_in(&text, dir.path()).unwrap();
        assert_eq!(config.plugins.entries["external.mine"].options["depth"], 3);
        assert_eq!(config.plugins.entries["external.mine"].enabled, Some(false));
    }

    #[test]
    fn set_rejects_unknown_keys_and_wrong_types_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "# keep me\n[diff]\ngraph_limit = 4\n").unwrap();
        let error = |key: &str, value: &str| set(&path, key, value).unwrap_err().to_string();
        assert_eq!(error("diff.typo", "1"), "diff.typo: unknown key");
        assert_eq!(
            error("diff.graph_limit", "abc"),
            "diff.graph_limit: expected an integer, got \"abc\""
        );
        assert_eq!(
            error("diff.graph_limit", "\"20\""),
            "diff.graph_limit: expected an integer, got \"\\\"20\\\"\""
        );
        assert!(error("diff.graph_limit", "-1").starts_with("diff.graph_limit: "));
        assert_eq!(error("diff", "1"), "diff is a table; set one of its keys");
        assert_eq!(error("", "1"), "invalid key \"\"");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# keep me\n[diff]\ngraph_limit = 4\n"
        );
    }

    #[test]
    fn values_are_read_as_the_setting_type_only() {
        let value = |schema: Value, text: &str| typed_value("k", &schema, text);
        let error = |schema: Value, text: &str| value(schema, text).unwrap_err().to_string();
        assert_eq!(
            value(json!({"type": "string"}), "12").unwrap(),
            toml::Value::String("12".into())
        );
        assert_eq!(
            value(json!({"type": ["string", "null"]}), "true").unwrap(),
            toml::Value::String("true".into())
        );
        assert_eq!(
            value(json!({"type": "integer"}), "12").unwrap(),
            toml::Value::Integer(12)
        );
        assert_eq!(
            error(json!({"type": "integer"}), "1.5"),
            "k: expected an integer, got \"1.5\""
        );
        assert_eq!(
            value(json!({"type": "number"}), "1.5").unwrap(),
            toml::Value::Float(1.5)
        );
        assert_eq!(
            value(json!({"type": "boolean"}), "false").unwrap(),
            toml::Value::Boolean(false)
        );
        assert_eq!(
            error(json!({"type": "boolean"}), "yes"),
            "k: expected true or false, got \"yes\""
        );
        assert_eq!(
            value(json!({"type": "array"}), "[\"a\", \"b\"]").unwrap(),
            toml::Value::Array(vec!["a".into(), "b".into()])
        );
        assert_eq!(
            error(json!({"type": "array"}), "12"),
            "k: expected a TOML array such as [\"a\", \"b\"], got \"12\""
        );
        assert!(error(json!({"type": "array"}), "a, b")
            .starts_with("k: expected a TOML array such as [\"a\", \"b\"], got \"a, b\": "));
        assert_eq!(
            value(json!({"enum": ["gemini"]}), "gemini").unwrap(),
            toml::Value::String("gemini".into())
        );
        assert_eq!(
            error(json!({"enum": ["gemini"]}), "openai"),
            "k: expected one of \"gemini\", got \"openai\""
        );
        assert_eq!(
            error(json!({"type": ["integer", "string"]}), "1"),
            "k: the schema gives it no single type, so config set cannot write it"
        );
        assert_eq!(
            error(json!({"type": "object"}), "{}"),
            "k: config set cannot write a value of type object"
        );
    }
}

#[cfg(test)]
mod materialization_tests {
    use super::*;

    #[test]
    fn first_edit_pins_defaults_and_later_edits_preserve_comments() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        set(&path, "diff.graph_limit", "42").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let raw: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(raw["version"].as_integer(), Some(1));
        assert_eq!(
            raw["plugins"]["order"].as_array().unwrap().len(),
            Config::default().plugins.order.len()
        );
        assert_eq!(
            raw["plugins"]["bundled"]["context"]["lines"].as_integer(),
            Some(3)
        );
        assert!(raw["plugins"]["bundled"]["summarize"]["system_prompt"]
            .as_str()
            .is_some());
        std::fs::write(&path, format!("# personal config\n{text}")).unwrap();
        set(&path, "plugins.bundled.context.lines", "8").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("# personal config\n"));
        assert_eq!(Config::from_toml(&text).unwrap().diff.graph_limit, 42);
    }

    #[test]
    fn materializing_an_explicit_list_does_not_restore_omitted_plugins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "# just grouping\n[plugins]\norder = ['bundled.group']\n",
        )
        .unwrap();
        set(&path, "diff.graph_limit", "42").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let config = Config::from_toml(&text).unwrap();
        assert_eq!(config.plugins.entries.len(), 1);
        assert!(text.contains("# just grouping\n[plugins]"), "{text}");
        assert!(!text.contains("bundled.context"));
    }
}

#[cfg(test)]
mod storage_settings_tests {
    use super::*;
    #[test]
    fn backend_and_directory_are_editable_config_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.toml");
        set(&path, "storage.backend", "file").unwrap();
        set(&path, "storage.path", ".cache/custom").unwrap();
        let shown = show(&Config::load(Some(&path)).unwrap(), false);
        assert_eq!(shown["storage"]["backend"], "file");
        assert_eq!(shown["storage"]["path"], ".cache/custom");
        set(&path, "storage.backend", "sqlite").unwrap();
        assert_eq!(
            show(&Config::load(Some(&path)).unwrap(), false)["storage"]["backend"],
            "sqlite"
        );
        assert!(set(&path, "storage.backend", "unknown").is_err());
    }
}
