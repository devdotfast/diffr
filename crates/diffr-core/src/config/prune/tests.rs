//! Pruning is a function of config text, so it is checked on every setting
//! the schema declares and on many generated files, against the properties
//! it promises rather than examples.
use super::*;
use crate::config::testing::{other, settings};
use crate::config::Config;
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

/// A small seeded generator, so every case is reproducible from its seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for index in (1..items.len()).rev() {
            items.swap(index, (self.next() % (index as u64 + 1)) as usize);
        }
    }
}

fn edit_value(value: &Value) -> toml_edit::Value {
    match value {
        Value::Bool(flag) => (*flag).into(),
        Value::Number(number) => match number.as_i64() {
            Some(integer) => integer.into(),
            None => number.as_f64().unwrap().into(),
        },
        Value::String(text) => text.as_str().into(),
        Value::Array(items) => toml_edit::Value::Array(items.iter().map(edit_value).collect()),
        _ => unreachable!("settings are scalars or arrays"),
    }
}

/// A config file of random settings at their default or another value, in
/// random order, with random comments, explicit headers and an old default
/// prompt.
fn generate(rng: &mut Rng, settings: &[(String, Value)]) -> (DocumentMut, Vec<String>) {
    let mut document = DocumentMut::new();
    document.insert("version", toml_edit::value(1));
    let mut chosen: Vec<(String, Value)> = Vec::new();
    for (key, node) in settings {
        if !rng.chance(40) {
            continue;
        }
        let default = node
            .get("default")
            .filter(|default| !default.is_null())
            .cloned();
        let value = match rng.chance(50) {
            true => default.or_else(|| other(node)),
            false => other(node).or(default),
        };
        if let Some(value) = value {
            chosen.push((key.clone(), value));
        }
    }
    if rng.chance(25) {
        let legacy = LEGACY_DEFAULTS[0].1[(rng.next() % 2) as usize];
        chosen.retain(|(key, _)| key != LEGACY_DEFAULTS[0].0);
        chosen.push((LEGACY_DEFAULTS[0].0.to_owned(), Value::from(legacy)));
    }
    rng.shuffle(&mut chosen);
    let mut comments = Vec::new();
    for (key, value) in chosen {
        let path: Vec<&str> = key.split('.').collect();
        let mut table = document.as_table_mut();
        for segment in &path[..path.len() - 1] {
            if !table.contains_key(segment) {
                let mut child = toml_edit::Table::new();
                child.set_implicit(rng.chance(50));
                if rng.chance(10) {
                    let comment = format!("# about {segment} {}\n", comments.len());
                    child.set_implicit(false);
                    child.decor_mut().set_prefix(comment.clone());
                    comments.push(comment.trim().to_owned());
                }
                table.insert(segment, Item::Table(child));
            }
            table = table[segment].as_table_mut().unwrap();
        }
        let leaf = path[path.len() - 1];
        let mut value = edit_value(&value);
        if let Some(first) = value.as_array_mut().and_then(|array| array.get_mut(0)) {
            if rng.chance(20) {
                let comment = format!("# in list {}", comments.len());
                first.decor_mut().set_prefix(format!("\n  {comment}\n  "));
                comments.push(comment);
            }
        }
        table.insert(leaf, toml_edit::value(value));
        if rng.chance(10) {
            let comment = format!("# note {}", comments.len());
            table
                .key_mut(leaf)
                .unwrap()
                .leaf_decor_mut()
                .set_prefix(format!("{comment}\n"));
            comments.push(comment);
        }
    }
    (document, comments)
}

fn resolve(text: &str) -> Option<toml::Value> {
    toml::Value::try_from(Config::from_toml_in(text, Path::new("")).ok()?).ok()
}

fn pruned(document: &DocumentMut) -> DocumentMut {
    let mut document = document.clone();
    prune(&mut document, resolve);
    document
}

fn document_leaves(document: &DocumentMut) -> BTreeSet<String> {
    removable(document.as_table(), false)
        .into_iter()
        .map(|path| path.join("."))
        .collect()
}

#[test]
fn pruning_generated_files_keeps_their_meaning_and_nothing_else() {
    let settings = settings();
    let mut cases = 0;
    for seed in 1..=400u64 {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let (document, comments) = generate(&mut rng, &settings);
        let input = document.to_string();
        let Some(meaning) = resolve(&input) else {
            continue;
        };
        cases += 1;
        let output = pruned(&document);
        let text = output.to_string();
        assert_eq!(
            resolve(&text).as_ref(),
            Some(&meaning),
            "seed {seed}: meaning\n{input}\n---\n{text}"
        );
        assert_eq!(
            pruned(&output).to_string(),
            text,
            "seed {seed}: idempotent\n{text}"
        );
        for path in removable(output.as_table(), false)
            .into_iter()
            .chain(removable(output.as_table(), true))
        {
            if path == ["version"] {
                continue;
            }
            let path: Vec<&str> = path.iter().map(String::as_str).collect();
            let mut candidate = output.clone();
            remove(candidate.as_table_mut(), &path);
            assert_ne!(
                resolve(&candidate.to_string()).as_ref(),
                Some(&meaning),
                "seed {seed}: {} could still go\n{text}",
                path.join(".")
            );
        }
        for comment in &comments {
            assert!(
                text.contains(comment.as_str()),
                "seed {seed}: lost {comment}\n{input}\n---\n{text}"
            );
        }
        let mut shuffled = DocumentMut::new();
        let mut rng = Rng(seed | 1);
        let mut lines: Vec<(String, toml_edit::Value)> = Vec::new();
        fn flatten(table: &dyn TableLike, prefix: &str, out: &mut Vec<(String, toml_edit::Value)>) {
            for (key, item) in table.iter() {
                let key = match prefix {
                    "" => key.to_owned(),
                    _ => format!("{prefix}.{key}"),
                };
                match item.as_table_like() {
                    Some(child) => flatten(child, &key, out),
                    None => out.push((key, item.as_value().unwrap().clone())),
                }
            }
        }
        flatten(document.as_table(), "", &mut lines);
        rng.shuffle(&mut lines);
        for (key, value) in lines {
            let path: Vec<&str> = key.split('.').collect();
            let mut table = shuffled.as_table_mut();
            for segment in &path[..path.len() - 1] {
                if !table.contains_key(segment) {
                    let mut child = toml_edit::Table::new();
                    child.set_implicit(true);
                    table.insert(segment, Item::Table(child));
                }
                table = table[segment].as_table_mut().unwrap();
            }
            let mut value = value;
            value.decor_mut().clear();
            table.insert(path[path.len() - 1], toml_edit::value(value));
        }
        let uncommented = |document: &DocumentMut| document_leaves(&pruned(document));
        if comments.is_empty() {
            assert_eq!(
                uncommented(&shuffled),
                document_leaves(&output),
                "seed {seed}: order\n{input}"
            );
        }
    }
    assert!(cases > 300, "{cases} generated files resolved");
}
