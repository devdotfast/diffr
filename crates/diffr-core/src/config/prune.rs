//! Keep the config file sparse, so that later default changes reach it.
//! Both passes remove keys by dotted path: [`prune`] each key whose removal
//! leaves the resolved configuration unchanged, and [`forget_legacy`] each
//! key that holds a default an earlier version wrote into files.
use std::borrow::Cow;
use toml_edit::{DocumentMut, Item, TableLike};

/// Defaults that versions before 0.1.8 wrote into config files on the first
/// edit, read as unset. Temporary: remove after 2026-12-31.
const LEGACY_DEFAULTS: &[(&str, &[&str])] = &[(
    "plugins.bundled.summarize.system_prompt",
    &[
        "For each listed fold, rewrite that function body as short python-flavored pseudocode. Keep the names. No prose, no comments, no code fences. Use as few lines as possible: about one pseudocode line per five source lines, and never more than a third of the body's lines. When a fold lists a doc, also set \"summary\" to one sentence copied verbatim from that doc; otherwise leave it empty. Answer with a JSON array of {\"id\", \"summary\", \"pseudocode\"} objects, one per fold.",
        "For each listed fold, rewrite that function body as short pseudocode. Keep the names. No prose, no comments, no code fences. Use as few lines as possible: about one pseudocode line per five source lines, and never more than a third of the body's lines. When a fold lists a doc, also set \"summary\" to one sentence copied verbatim from that doc; otherwise leave it empty. Answer with a JSON array of {\"id\", \"summary\", \"pseudocode\"} objects, one per fold.",
    ],
)];

/// `source` without the keys that hold a legacy default. Text that does not
/// parse is returned as it is, for the caller to report.
pub fn forget_legacy(source: &str) -> Cow<'_, str> {
    let Ok(mut document) = source.parse::<DocumentMut>() else {
        return Cow::Borrowed(source);
    };
    let mut changed = false;
    for (key, values) in LEGACY_DEFAULTS {
        let path: Vec<&str> = key.split('.').collect();
        let legacy = get(document.as_table(), &path)
            .and_then(Item::as_str)
            .is_some_and(|value| values.contains(&value));
        if legacy {
            remove(document.as_table_mut(), &path);
            changed = true;
        }
    }
    match changed {
        true => Cow::Owned(document.to_string()),
        false => Cow::Borrowed(source),
    }
}

/// Remove each value, then each table, whose removal leaves `resolve` of
/// the document unchanged: an object option is only valid whole. `version` stays, and so does
/// anything with a comment on it: a note marks intent.
pub fn prune(document: &mut DocumentMut, resolve: impl Fn(&str) -> Option<toml::Value>) {
    let Some(target) = resolve(&document.to_string()) else {
        return;
    };
    for tables in [false, true] {
        for path in removable(document.as_table(), tables) {
            if path == ["version"] {
                continue;
            }
            let path: Vec<&str> = path.iter().map(String::as_str).collect();
            let mut candidate = document.clone();
            remove(candidate.as_table_mut(), &path);
            if resolve(&candidate.to_string()).as_ref() == Some(&target) {
                *document = candidate;
            }
        }
    }
    hide_headers(document.as_table_mut());
}

/// Leave out the uncommented header of a table that only holds tables, such
/// as `[plugins]` above `[plugins.bundled.context]`.
fn hide_headers(table: &mut toml_edit::Table) {
    let only_tables = table.iter().all(|(_, item)| item.is_table());
    if only_tables && !table.is_empty() && !commented(table.decor()) {
        table.set_implicit(true);
    }
    for (_, item) in table.iter_mut() {
        if let Some(child) = item.as_table_mut() {
            hide_headers(child);
        }
    }
}

/// The dotted paths of the table's uncommented values or, with `tables`,
/// of its uncommented tables, deepest first. A table holding a comment at
/// any depth stays.
fn removable(table: &dyn TableLike, tables: bool) -> Vec<Vec<String>> {
    let mut paths = Vec::new();
    for (key, item) in table.iter() {
        if has_comment(table, key, item) {
            continue;
        }
        match item.as_table_like() {
            Some(child) => {
                for mut path in removable(child, tables) {
                    path.insert(0, key.to_owned());
                    paths.push(path);
                }
                if tables && !annotated(child) {
                    paths.push(vec![key.to_owned()]);
                }
            }
            None if !tables => paths.push(vec![key.to_owned()]),
            None => {}
        }
    }
    paths
}

/// Whether anything in the table, at any depth, has a comment.
fn annotated(table: &dyn TableLike) -> bool {
    table.iter().any(|(key, item)| {
        has_comment(table, key, item) || item.as_table_like().is_some_and(annotated)
    })
}

fn has_comment(table: &dyn TableLike, key: &str, item: &Item) -> bool {
    let key = table
        .key(key)
        .is_some_and(|key| commented(key.leaf_decor()));
    let item = match item {
        Item::Value(value) => value_has_comment(value),
        Item::Table(table) => commented(table.decor()),
        _ => false,
    };
    key || item
}

/// A comment on the value or anywhere inside it: between an array's
/// elements, after its last one, or on an inline table's entries.
fn value_has_comment(value: &toml_edit::Value) -> bool {
    let note = |raw: &toml_edit::RawString| raw.as_str().is_some_and(|text| text.contains('#'));
    commented(value.decor())
        || match value {
            toml_edit::Value::Array(array) => {
                note(array.trailing()) || array.iter().any(value_has_comment)
            }
            toml_edit::Value::InlineTable(table) => table.iter().any(|(key, value)| {
                table
                    .key(key)
                    .is_some_and(|key| commented(key.leaf_decor()))
                    || value_has_comment(value)
            }),
            _ => false,
        }
}

fn commented(decor: &toml_edit::Decor) -> bool {
    [decor.prefix(), decor.suffix()]
        .into_iter()
        .flatten()
        .any(|raw| raw.as_str().is_some_and(|text| text.contains('#')))
}

fn get<'a>(table: &'a dyn TableLike, path: &[&str]) -> Option<&'a Item> {
    match path {
        [key] => table.get(key),
        [key, rest @ ..] => get(table.get(key)?.as_table_like()?, rest),
        [] => None,
    }
}

fn remove(table: &mut dyn TableLike, path: &[&str]) {
    match path {
        [key] => {
            table.remove(key);
        }
        [key, rest @ ..] => {
            if let Some(child) = table.get_mut(key).and_then(Item::as_table_like_mut) {
                remove(child, rest);
            }
        }
        [] => {}
    }
}

#[cfg(test)]
mod tests;
