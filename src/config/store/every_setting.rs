//! `config set` on every setting the schema declares: a default is
//! dropped from the file and any other value kept.
use super::set;
use crate::config::testing::{other, settings};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

/// The text `config set` reads for `value`.
fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(Value::to_string).collect();
            format!("[{}]", items.join(", "))
        }
        other => other.to_string(),
    }
}

fn leaves(value: &toml::Value, prefix: &str, out: &mut BTreeSet<String>) {
    match value.as_table() {
        Some(table) => {
            for (key, child) in table {
                let key = match prefix {
                    "" => key.clone(),
                    _ => format!("{prefix}.{key}"),
                };
                leaves(child, &key, out);
            }
        }
        None => {
            out.insert(prefix.to_owned());
        }
    }
}

fn file_keys(path: &Path) -> BTreeSet<String> {
    let value: toml::Value = toml::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let mut keys = BTreeSet::new();
    leaves(&value, "", &mut keys);
    keys
}

#[test]
fn every_setting_is_dropped_at_its_default_and_kept_otherwise() {
    let settings = settings();
    assert!(
        settings.len() > 25,
        "{:?}",
        settings.iter().map(|(key, _)| key).collect::<Vec<_>>()
    );
    let mut checked = 0;
    for (key, node) in &settings {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        if let Some(default) = node.get("default").filter(|default| !default.is_null()) {
            if set(&path, key, &text(default)).is_ok() {
                assert_eq!(
                    file_keys(&path),
                    BTreeSet::from(["version".to_owned()]),
                    "{key} = default"
                );
                checked += 1;
            }
        }
        if let Some(value) = other(node) {
            let path = dir.path().join("other.toml");
            match set(&path, key, &text(&value)) {
                Ok(()) => {
                    assert_eq!(
                        file_keys(&path),
                        BTreeSet::from(["version".to_owned(), key.clone()]),
                        "{key} = {value}"
                    );
                    checked += 1;
                }
                // A value the configuration rejects is never written.
                Err(_) => assert!(!path.exists(), "{key}"),
            }
        }
    }
    assert!(checked > settings.len(), "{checked} of {}", settings.len());
}
