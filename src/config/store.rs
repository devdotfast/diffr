//! `diffr config`: read the schema and resolved values, and write one key to
//! the global file.
use super::{parse_value, Config, ConfigError};
use figment::providers::{Format, Serialized, Toml};
use figment::Figment;
use std::path::Path;

/// The resolved configuration as JSON, with the same nesting as the TOML.
/// `reveal` is accepted for secrets; no setting holds one yet.
pub(crate) fn show(config: &Config, _reveal: bool) -> serde_json::Value {
    serde_json::to_value(config).expect("config serializes")
}

/// Write `key = value` into the global file, keeping everything else in it
/// as written. The value is validated against the configuration before
/// anything touches the disk: unknown keys and mistyped values are errors.
pub(crate) fn set(path: &Path, key: &str, value: &str) -> Result<(), ConfigError> {
    if key.is_empty() || key.split('.').any(str::is_empty) {
        return Err(ConfigError(format!("invalid key {key:?}")));
    }
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(ConfigError(format!("{}: {error}", path.display()))),
    };
    let mut document: toml_edit::DocumentMut = existing
        .parse()
        .map_err(|error| ConfigError(format!("{}: {error}", path.display())))?;
    let typed = parse_value(value);
    let candidates = [
        Some(typed.clone()),
        (!matches!(typed, toml::Value::String(_))).then(|| toml::Value::String(value.to_owned())),
    ];
    let mut last_error = None;
    for candidate in candidates.into_iter().flatten() {
        let mut attempt = document.clone();
        assign(&mut attempt, key, &candidate)?;
        match validate(&attempt.to_string()) {
            Ok(()) => {
                document = attempt;
                last_error = None;
                break;
            }
            Err(error) => last_error = Some(error),
        }
    }
    if let Some(error) = last_error {
        return Err(error);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| ConfigError(format!("{}: {error}", parent.display())))?;
    }
    std::fs::write(path, document.to_string())
        .map_err(|error| ConfigError(format!("{}: {error}", path.display())))
}

fn validate(text: &str) -> Result<(), ConfigError> {
    Figment::from(Serialized::defaults(Config::default()))
        .merge(Toml::string(text))
        .extract::<Config>()
        .map(|_| ())
        .map_err(|error| ConfigError(error.to_string()))
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

    #[test]
    fn set_writes_typed_values_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        set(&path, "diff.graph_limit", "20").unwrap();
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
    fn set_rejects_unknown_keys_and_wrong_types_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "# keep me\n[diff]\ngraph_limit = 4\n").unwrap();
        assert!(set(&path, "diff.typo", "1").is_err());
        assert!(set(&path, "diff.graph_limit", "abc").is_err());
        assert!(set(&path, "diff", "1").is_err());
        assert!(set(&path, "", "1").is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# keep me\n[diff]\ngraph_limit = 4\n"
        );
    }
}
