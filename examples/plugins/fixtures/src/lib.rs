//! An example diffr plugin. `classify` tags a file `fixture` when it sits
//! under a `fixtures/` directory or its first line is `// fixture`, read from
//! the blob the file entry names: a plugin reads content itself, over the
//! `git` it already has, and so reads a deleted file as well as a new one.
//! `mutate` hides a fixture behind the subject of the last commit that
//! touched it, and collapses all but the first line of its first leaf, naming
//! the piece its cut creates by predicting the id.
use diffr_plugin_sdk::apply::Fresh;
use diffr_plugin_sdk::types::Cut;
use diffr_plugin_sdk::{
    anyhow, export, host, line_count, FileEntry, Move, Node, Pairing, Plugin, Source, ROOT,
};
use serde::Deserialize;

const TAG: &str = "fixture";
const MARKER: &str = "// fixture\n";
/// Git's mode for a plain file; a symlink or a submodule has another.
const REGULAR: &str = "100644";

/// `git`, with its stderr as the error.
fn git(args: &[&str]) -> anyhow::Result<String> {
    let args: Vec<String> = args.iter().map(|arg| (*arg).to_owned()).collect();
    host::git(&args).map_err(|stderr| anyhow::anyhow!("git {}: {stderr}", args[0]))
}

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
        let side = file.side();
        if file.path().starts_with("fixtures/") || file.path().contains("/fixtures/") {
            return Ok(vec![TAG.to_owned()]);
        }
        // Only a regular file has a first line to read; a symlink or a
        // submodule is not a fixture.
        if side.mode != REGULAR {
            return Ok(Vec::new());
        }
        // The blob, not the working tree: a deleted file has content too, and
        // it is the content that was diffed either way.
        let text = git(&["cat-file", "blob", &side.oid])?;
        Ok(match text.starts_with(MARKER) {
            true => vec![TAG.to_owned()],
            false => Vec::new(),
        })
    }

    fn mutate(&self, file: &FileEntry, sides: &Pairing<Source>) -> anyhow::Result<Vec<Move>> {
        anyhow::ensure!(!self.options.fail, "asked to fail on {}", file.path());
        if !file.tags.iter().any(|tag| tag == TAG) {
            return Ok(Vec::new());
        }
        let subject = git(&["log", "-1", "--format=%s", "--", file.path()])?;
        let subject = subject.trim();
        eprintln!("{}: last changed in {subject:?}", file.path());
        let mut moves = vec![
            Move::SetCollapsed((ROOT, true)),
            Move::SetLabel((ROOT, Some(format!("Fixture · {subject}")))),
        ];
        // The first top-level leaf of two or more lines, on the before side
        // when there is one. A cut hands out its piece ids lhs first, so the
        // piece on this leaf's side takes the first fresh id either way.
        let side = match &sides {
            Pairing::Both { lhs, .. } | Pairing::LeftOnly { lhs } => lhs,
            Pairing::RightOnly { rhs } => rhs,
        };
        let leaf = side
            .regions
            .iter()
            .find(|region| matches!(region.node, Node::Leaf { .. }) && line_count(region) >= 2);
        if let Some(leaf) = leaf {
            let piece = Fresh::of(sides).id();
            moves.extend([
                Move::Cut(Cut {
                    region: leaf.id,
                    at: 1,
                }),
                Move::SetCollapsed((piece, true)),
                Move::SetLabel((
                    piece,
                    Some(format!("{} fixture lines", line_count(leaf) - 1)),
                )),
            ]);
        }
        Ok(moves)
    }
}

export!(Fixtures);
