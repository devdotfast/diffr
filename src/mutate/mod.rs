//! Mutations run on the wire types after projection and before a record is
//! written: file mutations on the manifest, fold mutations on each file's
//! region trees. diffr's diff internals never see them.
//!
//! Order: file mutations (hidden categories), then fold mutations in this
//! sequence: deleted function bodies, test bodies, removed runs, the
//! built-in summarizer, the JSON-RPC hook, and last the grouping pass that
//! merges adjacent gaps and wraps runs of collapsed folds. A mutation
//! failure aborts the run; retries belong inside a mutation.
pub(crate) mod collapse;
pub(crate) mod group;
pub(crate) mod summarize;
use crate::config::Params;
use crate::hash::DftHashSet;
use crate::hook::Hook;
use crate::protocol::{FileChange, Node, Pairing, Problem, Region, Source, Visibility};
use std::path::Path;

pub(crate) trait FileMutation: Send + Sync {
    fn apply(&self, file: &mut FileChange) -> Result<(), Problem>;
}

pub(crate) trait FoldMutation: Send + Sync {
    fn apply(&self, file: &FileChange, sides: &mut Pairing<Source>) -> Result<(), Problem>;
}

/// The enabled mutations, in the order they run.
#[derive(Default)]
pub(crate) struct Mutations {
    file: Vec<Box<dyn FileMutation>>,
    fold: Vec<Box<dyn FoldMutation>>,
}

impl Mutations {
    /// Build the registry from configuration. Starting the hook or the
    /// summarizer client can fail, which is a setup error before `start`.
    pub(crate) fn from_params(params: &Params, workspace: &Path) -> crate::git::Result<Self> {
        let mut mutations = Self::default();
        let folds = &params.folds;
        if folds.collapse_deleted_files || folds.collapse_generated || folds.collapse_tests {
            mutations.file.push(Box::new(collapse::HiddenCategories {
                deleted: folds.collapse_deleted_files,
                generated: folds.collapse_generated,
                tests: folds.collapse_tests,
            }));
        }
        if folds.collapse_deleted {
            mutations.fold.push(Box::new(collapse::DeletedBodies {
                min_lines: folds.min_lines,
            }));
        }
        if folds.collapse_test_bodies {
            mutations.fold.push(Box::new(collapse::TestBodies));
        }
        if folds.collapse_removed_lines > 0 {
            mutations.fold.push(Box::new(collapse::RemovedRuns {
                min_lines: folds.collapse_removed_lines,
            }));
        }
        if params.summarize.enabled {
            if let Some(summarizer) = summarize::Summarizer::new(&params.summarize)? {
                mutations.fold.push(Box::new(summarizer));
            }
        }
        if let Some(hook) = &params.hook {
            mutations
                .fold
                .push(Box::new(Hook::spawn(hook, folds.min_lines, workspace)?));
        }
        mutations.fold.push(Box::new(group::GroupCollapsed));
        Ok(mutations)
    }

    pub(crate) fn apply_file(&self, file: &mut FileChange) -> Result<(), Problem> {
        for mutation in &self.file {
            mutation.apply(file)?;
        }
        Ok(())
    }

    pub(crate) fn apply_fold(
        &self,
        file: &FileChange,
        sides: &mut Pairing<Source>,
    ) -> Result<(), Problem> {
        for mutation in &self.fold {
            mutation.apply(file, sides)?;
        }
        Ok(())
    }
}

// ── region helpers ────────────────────────────────────────────────────────

/// Visit every region, parents before children.
pub(crate) fn walk_mut(regions: &mut [Region], visit: &mut impl FnMut(&mut Region)) {
    for region in regions {
        visit(region);
        if let Node::Fold { children } = &mut region.node {
            walk_mut(children, visit);
        }
    }
}

pub(crate) fn walk(regions: &[Region], visit: &mut impl FnMut(&Region)) {
    for region in regions {
        visit(region);
        if let Node::Fold { children } = &region.node {
            walk(children, visit);
        }
    }
}

/// Every alignment id on a side, to tell one-sided regions from paired ones.
pub(crate) fn ids(regions: &[Region]) -> DftHashSet<u32> {
    let mut ids = DftHashSet::default();
    walk(regions, &mut |region| {
        ids.insert(region.alignment_id);
    });
    ids
}

/// True when nothing under `region`, itself included, has a counterpart on
/// the other side. A fold whose header line did not align can still hold
/// paired leaves, and such a fold is a rewrite, not a removal.
pub(crate) fn one_sided(region: &Region, other_ids: &DftHashSet<u32>) -> bool {
    let mut paired = false;
    walk(std::slice::from_ref(region), &mut |inner| {
        paired |= other_ids.contains(&inner.alignment_id);
    });
    !paired
}

pub(crate) fn line_count(region: &Region) -> usize {
    region.range.lines().len()
}

pub(crate) fn is_fold(region: &Region) -> bool {
    matches!(region.node, Node::Fold { .. })
}

/// The line-comment marker for a language's display name, `#` when unknown.
pub(crate) fn comment_marker(language: Option<&str>) -> &'static str {
    match language.unwrap_or_default() {
        "C" | "C++" | "C#" | "Objective-C" | "Rust" | "Go" | "Java" | "Kotlin" | "Swift"
        | "Scala" | "Dart" | "JavaScript" | "JavaScript JSX" | "TypeScript" | "TypeScript TSX"
        | "QML" | "PHP" | "Zig" | "Solidity" | "Proto" | "Verilog" | "Smali" | "Gleam" | "F#"
        | "Apex" => "//",
        "Lua" | "SQL" | "Haskell" | "Elm" | "Ada" | "VHDL" | "OCaml" | "OCaml Interface" => "--",
        "Common Lisp" | "Emacs Lisp" | "Clojure" | "Scheme" | "Racket" | "Janet" | "Assembly" => {
            ";"
        }
        "Erlang" | "LaTeX" => "%",
        "Pascal" => "//",
        _ => "#",
    }
}

/// The collapsed label for a summary: a comment line saying it is
/// pseudocode, then the text.
pub(crate) fn summary_label(language: Option<&str>, text: &str) -> String {
    format!(
        "{} pseudocode\n{}",
        comment_marker(language),
        text.trim_end()
    )
}

pub(crate) fn collapse(region: &mut Region, label: String) {
    region.visibility = Visibility {
        collapsed: true,
        label,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comment_markers_follow_the_language() {
        assert_eq!(comment_marker(Some("Python")), "#");
        assert_eq!(comment_marker(Some("Rust")), "//");
        assert_eq!(comment_marker(Some("TypeScript TSX")), "//");
        assert_eq!(comment_marker(Some("Lua")), "--");
        assert_eq!(comment_marker(Some("Clojure")), ";");
        assert_eq!(comment_marker(Some("Erlang")), "%");
        assert_eq!(comment_marker(None), "#");
        assert_eq!(
            summary_label(Some("Go"), "x = 1\nreturn x\n"),
            "// pseudocode\nx = 1\nreturn x"
        );
    }
}
