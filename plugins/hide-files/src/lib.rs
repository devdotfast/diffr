//! Hide whole files by their tags or status.
use diffr_plugin_sdk::types::Source;
use diffr_plugin_sdk::{anyhow, export, FileEntry, FileStatus, Move, Plugin, ROOT};
use serde::Deserialize;

/// A deleted file, when `deleted` is set, or a file carrying one of `tags`
/// starts hidden. The reason names the status or the first listed tag the
/// file carries: "Deleted file · hidden by default", "Generated file ·
/// hidden by default".
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    tags: Vec<String>,
    deleted: bool,
}

pub struct HideFiles {
    options: Options,
}

impl Plugin for HideFiles {
    type Options = Options;

    fn new(options: Options) -> anyhow::Result<Self> {
        Ok(Self { options })
    }

    fn classify(&self, _file: &FileEntry) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn mutate(
        &self,
        file: &FileEntry,
        _lhs: Option<&Source>,
        _rhs: Option<&Source>,
    ) -> anyhow::Result<Vec<Move>> {
        let options = &self.options;
        let what = if options.deleted && file.status == FileStatus::Deleted {
            "Deleted".to_owned()
        } else {
            match options.tags.iter().find(|tag| file.tags.contains(tag)) {
                Some(tag) => capitalized(tag),
                None => return Ok(Vec::new()),
            }
        };
        Ok(vec![
            Move::SetCollapsed {
                region: ROOT,
                collapsed: true,
            },
            Move::SetLabel {
                region: ROOT,
                label: Some(format!("{what} file · hidden by default")),
            },
        ])
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

export!(HideFiles);
