//! Collapse function bodies that were deleted.
use diffr_plugin_sdk::types::Source;
use diffr_plugin_sdk::{
    anyhow, before_and_after_ids, docstring_of, export, has_tag, is_fold, line_count, one_sided,
    tree, walk, Draft, FileEntry, Move, Plugin,
};
use serde::Deserialize;

/// The plugin's name, and the tag its queries set on function bodies.
const PLUGIN: &str = "deleted-bodies";
const FUNCTION: &str = "deleted-bodies:function";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    min_lines: usize,
}

/// Deleted function bodies of at least `min_lines` start collapsed with a
/// line count, linked to their docstrings, which collapse with them. A body
/// counts as deleted only when nothing under it is paired: a fold whose lines
/// still align with the after side is a rewrite, not a removal.
pub struct DeletedBodies {
    options: Options,
}

impl Plugin for DeletedBodies {
    type Options = Options;

    fn new(options: Options) -> anyhow::Result<Self> {
        Ok(Self { options })
    }

    fn classify(&self, _file: &FileEntry) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn mutate(
        &self,
        _file: &FileEntry,
        lhs: Option<&Source>,
        rhs: Option<&Source>,
    ) -> anyhow::Result<Vec<Move>> {
        let options = &self.options;
        let sides = tree::sides(lhs, rhs)?;
        let Some((lhs, rhs)) = before_and_after_ids(&sides) else {
            return Ok(Vec::new());
        };
        // Each deleted body: its id, line count and docstring.
        let mut bodies = Vec::new();
        walk(&lhs.regions, &mut |region| {
            let count = line_count(region);
            if is_fold(region)
                && has_tag(region, FUNCTION)
                && one_sided(region, &rhs)
                && count >= options.min_lines
            {
                bodies.push((region.id, count, docstring_of(lhs, region, PLUGIN)));
            }
        });
        let mut draft = Draft::new(&sides);
        for (id, count, docstring) in bodies {
            draft.collapse(id, format!("{count} lines removed"))?;
            if let Some(docstring) = docstring {
                draft.link(&[id, docstring])?;
            }
        }
        Ok(draft.into_moves())
    }
}

export!(DeletedBodies);
