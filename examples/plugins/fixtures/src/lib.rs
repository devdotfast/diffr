//! An example diffr plugin. `classify` tags a file `fixture` when it sits
//! under a `fixtures/` directory or its first line is `// fixture`, read with
//! the host's `read_head`. `mutate` hides a fixture behind the subject of the
//! last commit that touched it, asked of the host's `git`, and collapses all
//! but the first line of its first leaf, naming the piece its cut creates by
//! predicting the id.
use diffr_plugin_sdk::apply::Fresh;
use diffr_plugin_sdk::types::Source;
use diffr_plugin_sdk::{
    anyhow, export, host, line_count, tree, FileEntry, FileStatus, Move, Node, Pairing, Plugin,
    Side, ROOT,
};
use serde::Deserialize;

const TAG: &str = "fixture";
const MARKER: &[u8] = b"// fixture\n";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    fail: bool,
}

pub struct Fixtures {
    options: Options,
}

impl Plugin for Fixtures {
    type Options = Options;

    fn new(options: Options) -> anyhow::Result<Self> {
        Ok(Self { options })
    }

    fn classify(&self, file: &FileEntry) -> anyhow::Result<Vec<String>> {
        if file.path.starts_with("fixtures/") || file.path.contains("/fixtures/") {
            return Ok(vec![TAG.to_owned()]);
        }
        let side = match file.status {
            FileStatus::Deleted => Side::Lhs,
            _ => Side::Rhs,
        };
        let head = host::read_head(side, MARKER.len() as u32);
        Ok(match head.as_deref() == Some(MARKER) {
            true => vec![TAG.to_owned()],
            false => Vec::new(),
        })
    }

    fn mutate(
        &self,
        file: &FileEntry,
        lhs: Option<&Source>,
        rhs: Option<&Source>,
    ) -> anyhow::Result<Vec<Move>> {
        anyhow::ensure!(!self.options.fail, "asked to fail on {}", file.path);
        if !file.tags.iter().any(|tag| tag == TAG) {
            return Ok(Vec::new());
        }
        let args = ["log", "-1", "--format=%s", "--", &file.path].map(str::to_owned);
        let subject = host::git(&args).map_err(|stderr| anyhow::anyhow!("git log: {stderr}"))?;
        let subject = subject.trim();
        host::log(&format!("{}: last changed in {subject:?}", file.path));
        let mut moves = vec![
            Move::SetCollapsed {
                region: ROOT,
                collapsed: true,
            },
            Move::SetLabel {
                region: ROOT,
                label: Some(format!("Fixture · {subject}")),
            },
        ];
        // The first top-level leaf of two or more lines, on the before side
        // when there is one. A cut hands out its piece ids lhs first, so the
        // piece on this leaf's side takes the first fresh id either way.
        let sides = tree::sides(lhs, rhs)?;
        let side = match &sides {
            Pairing::Both { lhs, .. } | Pairing::LeftOnly { lhs } => lhs,
            Pairing::RightOnly { rhs } => rhs,
        };
        let leaf = side
            .regions
            .iter()
            .find(|region| matches!(region.node, Node::Leaf { .. }) && line_count(region) >= 2);
        if let Some(leaf) = leaf {
            let piece = Fresh::of(&sides).id();
            moves.extend([
                Move::Cut {
                    region: leaf.id,
                    at: 1,
                },
                Move::SetCollapsed {
                    region: piece,
                    collapsed: true,
                },
                Move::SetLabel {
                    region: piece,
                    label: Some(format!("{} fixture lines", line_count(leaf) - 1)),
                },
            ]);
        }
        Ok(moves)
    }
}

export!(Fixtures);
