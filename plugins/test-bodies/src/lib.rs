//! Collapse test bodies and test modules.
use diffr_plugin_sdk::{
    anyhow, docstring_of, export, has_tag, is_fold, line_count, walk, Draft, FileEntry, Move,
    Pairing, Plugin, Source,
};
use serde::Deserialize;
use std::collections::BTreeMap;

/// The plugin's name, and the tags its queries set: a test function's body,
/// and a test module's body.
const PLUGIN: &str = "test-bodies";
const TEST: &str = "test-bodies:test";
const MODULE: &str = "test-bodies:module";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    min_lines: usize,
}

/// Test bodies of at least `min_lines` start collapsed on both sides, paired
/// or not, so a diff reads as the code under test first. A whole test
/// module, such as a Rust `#[cfg(test)] mod tests`, collapses as one fold
/// labelled "test module"; the header stays visible and the fold expands
/// like any other. A test body is linked to its docstring, which collapses
/// with it.
pub struct TestBodies {
    options: Options,
}

impl Plugin for TestBodies {
    type Options = Options;

    fn new(options: Options) -> anyhow::Result<Self> {
        Ok(Self { options })
    }

    fn queries(&self) -> anyhow::Result<Vec<diffr_plugin_sdk::QuerySource>> {
        Ok(vec![
            diffr_plugin_sdk::QuerySource {
                language: "rust".into(),
                name: "builtin:test-bodies/queries/rust.scm".into(),
                text: include_str!("../queries/rust.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "python".into(),
                name: "builtin:test-bodies/queries/python.scm".into(),
                text: include_str!("../queries/python.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "go".into(),
                name: "builtin:test-bodies/queries/go.scm".into(),
                text: include_str!("../queries/go.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "javascript".into(),
                name: "builtin:test-bodies/queries/javascript.scm".into(),
                text: include_str!("../queries/javascript.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "javascriptjsx".into(),
                name: "builtin:test-bodies/queries/javascript.scm".into(),
                text: include_str!("../queries/javascript.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "typescript".into(),
                name: "builtin:test-bodies/queries/javascript.scm".into(),
                text: include_str!("../queries/javascript.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "typescripttsx".into(),
                name: "builtin:test-bodies/queries/javascript.scm".into(),
                text: include_str!("../queries/javascript.scm").into(),
            },
        ])
    }

    fn classify(&self, _file: &FileEntry) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn mutate(&self, _file: &FileEntry, sides: &Pairing<Source>) -> anyhow::Result<Vec<Move>> {
        let options = &self.options;
        // Every fold has its own id, so each side's fold is its own target
        // and takes its own label. A test body's docstring, when it has one,
        // is linked after it collapses.
        let mut labels: BTreeMap<u32, (String, Option<u32>)> = BTreeMap::new();
        for source in sides.sides() {
            walk(&source.regions, &mut |region| {
                if !is_fold(region) || line_count(region) < options.min_lines {
                    return;
                }
                if has_tag(region, MODULE) {
                    labels.insert(
                        region.id,
                        (
                            if region.visibility.label.is_empty() {
                                "test module".to_owned()
                            } else {
                                region.visibility.label.clone()
                            },
                            None,
                        ),
                    );
                } else if has_tag(region, TEST) {
                    labels.entry(region.id).or_insert((
                        if region.visibility.label.is_empty() {
                            "test body".to_owned()
                        } else {
                            region.visibility.label.clone()
                        },
                        docstring_of(source, region, PLUGIN),
                    ));
                }
            });
        }
        let mut draft = Draft::new(sides);
        for (id, (label, docstring)) in labels {
            draft.collapse(id, label.to_owned())?;
            if let Some(docstring) = docstring {
                draft.link(&[id, docstring])?;
            }
        }
        Ok(draft.into_moves())
    }
}

export!("test-bodies", TestBodies);
