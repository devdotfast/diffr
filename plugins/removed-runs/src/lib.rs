//! Collapse the middle of long removed stretches.
use diffr_plugin_sdk::{
    anyhow, before_and_after_ids, export, has_search_highlights, has_tag, line_count, one_sided,
    Draft, FileEntry, Move, Node, OtherSide, Pairing, Plugin, Region, Source,
};
use serde::Deserialize;

/// The tag this plugin's queries set on function bodies.
const FUNCTION: &str = "removed-runs:function";

/// Removed stretches with no counterpart and at least `min_lines` lines
/// (never fewer than three) keep their first and last line open and
/// collapse the rest with a line count, so the reader still sees red at both
/// ends. Only stretches in unpaired code qualify: the nearest enclosing
/// function fold (or, outside any function, the nearest enclosing fold)
/// must itself be one-sided, so a rewritten function shows its red and green
/// lines in place. Stretches at the top level qualify. Leaves that already
/// start collapsed, or sit under a fold that does, are left alone.
pub struct RemovedRuns {
    options: Options,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    min_lines: usize,
}

impl Plugin for RemovedRuns {
    type Options = Options;

    fn new(options: Options) -> anyhow::Result<Self> {
        Ok(Self { options })
    }

    fn queries(&self) -> anyhow::Result<Vec<diffr_plugin_sdk::QuerySource>> {
        Ok(vec![
            diffr_plugin_sdk::QuerySource {
                language: "rust".into(),
                name: "builtin:removed-runs/queries/rust.scm".into(),
                text: include_str!("../queries/rust.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "python".into(),
                name: "builtin:removed-runs/queries/python.scm".into(),
                text: include_str!("../queries/python.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "go".into(),
                name: "builtin:removed-runs/queries/go.scm".into(),
                text: include_str!("../queries/go.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "javascript".into(),
                name: "builtin:removed-runs/queries/javascript.scm".into(),
                text: include_str!("../queries/javascript.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "javascriptjsx".into(),
                name: "builtin:removed-runs/queries/javascript.scm".into(),
                text: include_str!("../queries/javascript.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "typescript".into(),
                name: "builtin:removed-runs/queries/javascript.scm".into(),
                text: include_str!("../queries/javascript.scm").into(),
            },
            diffr_plugin_sdk::QuerySource {
                language: "typescripttsx".into(),
                name: "builtin:removed-runs/queries/javascript.scm".into(),
                text: include_str!("../queries/javascript.scm").into(),
            },
        ])
    }

    fn classify(&self, _file: &FileEntry) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn mutate(&self, _file: &FileEntry, sides: &Pairing<Source>) -> anyhow::Result<Vec<Move>> {
        let options = &self.options;
        let Some((lhs, rhs)) = before_and_after_ids(sides) else {
            return Ok(Vec::new());
        };
        // A leaf needs a first, a middle, and a last line.
        let threshold = options.min_lines.max(3);
        let mut leaves = Vec::new();
        visit(
            &lhs.regions,
            &rhs,
            threshold,
            false,
            Gates::default(),
            &mut leaves,
        );
        let mut draft = Draft::new(sides);
        for (id, len) in leaves {
            let middle = draft.cut_lines(id, 1, len - 1)?;
            draft.collapse(middle, format!("{} lines removed", len - 2))?;
        }
        Ok(draft.into_moves())
    }
}

/// Whether the enclosing folds are one-sided: the nearest function fold,
/// and the nearest fold of any kind. `None` when there is no such fold.
#[derive(Clone, Copy, Default)]
struct Gates {
    function: Option<bool>,
    any: Option<bool>,
}

impl Gates {
    fn enter(self, fold: &Region, rhs: &OtherSide) -> Self {
        let unpaired = one_sided(fold, rhs);
        Self {
            function: if has_tag(fold, FUNCTION) {
                Some(unpaired)
            } else {
                self.function
            },
            any: Some(unpaired),
        }
    }

    /// Inside a paired function nothing collapses; outside any function the
    /// nearest fold decides; at the top level everything qualifies.
    fn open(self) -> bool {
        self.function.or(self.any).unwrap_or(true)
    }
}

/// Every leaf whose middle collapses, with its line count, in document
/// order.
fn visit(
    regions: &[Region],
    rhs: &OtherSide,
    threshold: usize,
    under_collapsed: bool,
    gates: Gates,
    leaves: &mut Vec<(u32, u32)>,
) {
    for region in regions {
        let collapsed = under_collapsed || region.visibility.collapsed;
        match &region.node {
            Node::Fold { children } => {
                let inner = gates.enter(region, rhs);
                visit(children, rhs, threshold, collapsed, inner, leaves);
            }
            Node::Leaf { .. } => {
                let len = line_count(region);
                if !collapsed
                    && !has_search_highlights(region)
                    && gates.open()
                    && !rhs.pairs(region)
                    && len >= threshold
                {
                    leaves.push((region.id, len as u32));
                }
            }
        }
    }
}

export!("removed-runs", RemovedRuns);
