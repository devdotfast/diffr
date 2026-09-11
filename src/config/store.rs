//! `diffr config`: read the schema and resolved values, and write one key to
//! the global file.
use super::{parse_value, Config, ConfigError};
use figment::providers::{Format, Serialized, Toml};
use figment::Figment;
use std::path::Path;

/// The resolved configuration as JSON, with the same nesting as the TOML.
/// The API key is redacted unless `reveal` is set.
pub(crate) fn show(config: &Config, reveal: bool) -> serde_json::Value {
    let mut value = serde_json::to_value(config).expect("config serializes");
    if !reveal && config.summarize.api_key.is_some() {
        value["summarize"]["api_key"] = serde_json::Value::String("<redacted>".to_owned());
    }
    value
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
        set(&path, "folds.min_lines", "20").unwrap();
        set(&path, "summarize.api_key", "1234").unwrap();
        set(&path, "folds.collapse_tests", "false").unwrap();
        set(&path, "summarize.model", "gemini-x").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("min_lines = 20"), "{text}");
        assert!(text.contains("api_key = \"1234\""), "{text}");
        assert!(text.contains("collapse_tests = false"), "{text}");
        assert!(text.contains("model = \"gemini-x\""), "{text}");
        let config = Config::from_toml(&text).unwrap();
        assert_eq!(config.folds.min_lines, 20);
        assert_eq!(config.summarize.api_key.as_deref(), Some("1234"));
    }

    #[test]
    fn set_rejects_unknown_keys_and_wrong_types_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "# keep me\n[folds]\nmin_lines = 4\n").unwrap();
        assert!(set(&path, "folds.typo", "1").is_err());
        assert!(set(&path, "folds.min_lines", "abc").is_err());
        assert!(set(&path, "folds", "1").is_err());
        assert!(set(&path, "", "1").is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "# keep me\n[folds]\nmin_lines = 4\n"
        );
    }

    #[test]
    fn show_redacts_the_key_unless_revealed() {
        let mut config = Config::default();
        config.summarize.api_key = Some("secret".to_owned());
        assert_eq!(show(&config, false)["summarize"]["api_key"], "<redacted>");
        assert_eq!(show(&config, true)["summarize"]["api_key"], "secret");
        assert!(show(&Config::default(), false)["summarize"]["api_key"].is_null());
        assert_eq!(show(&config, false)["folds"]["min_lines"], 12);
    }
}
