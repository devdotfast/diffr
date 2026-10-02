//! What tests of the configuration share: every setting the schema
//! declares, and a value for each other than its default. A crate that tests
//! against the configuration gets it with the `test-support` feature.
use super::Config;
use serde_json::Value;

/// Every setting the schema declares, with its schema node.
pub fn settings() -> Vec<(String, Value)> {
    let root = Config::schema();
    let mut out = Vec::new();
    fn walk(node: &Value, root: &Value, key: &str, out: &mut Vec<(String, Value)>) {
        let node = match node.get("$ref").and_then(Value::as_str) {
            Some(reference) => &root["$defs"][reference.rsplit('/').next().unwrap()],
            None => node,
        };
        match node.get("properties").and_then(Value::as_object) {
            // An object option is set whole, from the file, never by field.
            Some(_) if node.get("x-settings") == Some(&Value::Bool(false)) => {}
            Some(properties) => {
                for (name, child) in properties {
                    let key = match key {
                        "" => name.clone(),
                        _ => format!("{key}.{name}"),
                    };
                    walk(child, root, &key, out);
                }
            }
            None => out.push((key.to_owned(), node.clone())),
        }
    }
    walk(&root, &root, "", &mut out);
    out.retain(|(key, node)| {
        key != "version"
            && !key.starts_with("plugins.external")
            && node.get("type").and_then(Value::as_str) != Some("object")
    });
    out
}

/// A valid value other than the default, when the schema allows one.
pub fn other(node: &Value) -> Option<Value> {
    let default = node.get("default");
    if let Some(choices) = node.get("enum").and_then(Value::as_array) {
        return choices
            .iter()
            .find(|choice| Some(*choice) != default)
            .cloned();
    }
    let kind = match node.get("type") {
        Some(Value::Array(kinds)) => kinds.iter().find(|kind| *kind != "null")?.as_str()?,
        Some(kind) => kind.as_str()?,
        None => return None,
    };
    Some(match kind {
        "boolean" => Value::Bool(!default.and_then(Value::as_bool).unwrap_or(false)),
        "integer" => {
            let minimum = node.get("minimum").and_then(Value::as_i64).unwrap_or(0);
            let maximum = node
                .get("maximum")
                .and_then(Value::as_i64)
                .unwrap_or(i64::MAX);
            let base = default.and_then(Value::as_i64).unwrap_or(minimum);
            Value::from(if base < maximum { base + 1 } else { base - 1 })
        }
        "number" => Value::from(default.and_then(Value::as_f64).unwrap_or(0.0) + 0.5),
        "string" => Value::from("custom-value"),
        "array" => match default.and_then(Value::as_array) {
            Some(items) if items.len() > 1 => Value::Array(items.iter().rev().cloned().collect()),
            Some(items) if !items.is_empty() => Value::Array(Vec::new()),
            _ => return None,
        },
        _ => return None,
    })
}
