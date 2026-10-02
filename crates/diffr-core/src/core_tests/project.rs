//! The projection's tests, against the bundled plugins' fold queries.
#![allow(unused_imports)]
use crate::config::body_params;
use crate::hash::DftHashMap;
use crate::line_layout::{aligned_rows, novel_lines, runs, Run};
use crate::line_parser;
use crate::pairing::Pairing;
use crate::params::DiffOptions;
use crate::parse::folds::{self, Fold, FoldMatch};
use crate::parse::syntax::{MatchKind, MatchedPos, SyntaxId};
use crate::parse::tree_sitter_parser::{highlight_captures, TreeSitterConfig};
use crate::protocol::project::*;
use crate::protocol::{
    BinaryRef, Diff, FileRef, LineCounts, Node, Problem, Region, Source, SourcePos, SourceRange,
    Span, Stats, SyntaxSpan, Visibility, ROOT,
};
use crate::summary::{DiffResult, FallbackCause, FileContent, FileFormat};
use std::collections::{BTreeMap, BTreeSet};

fn refs(lhs: bool, rhs: bool) -> Pairing<FileRef> {
    let file_ref = FileRef {
        path: "a.py".to_owned(),
        oid: String::new(),
        mode: String::new(),
    };
    match (lhs, rhs) {
        (true, true) => Pairing::Both {
            lhs: file_ref.clone(),
            rhs: file_ref,
        },
        (true, false) => Pairing::LeftOnly { lhs: file_ref },
        (false, true) => Pairing::RightOnly { rhs: file_ref },
        (false, false) => panic!("a file has a side"),
    }
}

fn project(path: &str, lhs: &str, rhs: &str) -> Diff {
    project_with(path, lhs, rhs, DiffOptions::default())
}

/// `graph_limit: 1` forces the text-diff fallback for any real change.
fn project_with(path: &str, lhs: &str, rhs: &str, options: DiffOptions) -> Diff {
    let result =
        DiffResult::from_sources_with_options(path, lhs, rhs, &body_params(), &options).unwrap();
    diff(
        &result,
        Inputs {
            file: &refs(!lhs.is_empty(), !rhs.is_empty()),
            sizes: (lhs.len() as u64, rhs.len() as u64),
            syntax: (Vec::new(), Vec::new()),
        },
    )
}

fn sources(diff: &Diff) -> (Option<&Source>, Option<&Source>) {
    match diff {
        Diff::Text { sides, .. } => match sides {
            Pairing::Both { lhs, rhs } => (Some(lhs), Some(rhs)),
            Pairing::LeftOnly { lhs } => (Some(lhs), None),
            Pairing::RightOnly { rhs } => (None, Some(rhs)),
        },
        Diff::Binary { .. } => panic!("text diff"),
    }
}

fn leaves(regions: &[Region]) -> Vec<&Region> {
    let mut out = Vec::new();
    for region in regions {
        match &region.node {
            Node::Leaf { .. } => out.push(region),
            Node::Fold { children } => out.extend(leaves(children)),
        }
    }
    out
}

fn alignment(leaf: &Region) -> u32 {
    match leaf.node {
        Node::Leaf { alignment_id, .. } => alignment_id,
        Node::Fold { .. } => panic!("a fold has no alignment_id: {leaf:?}"),
    }
}

fn all(regions: &[Region]) -> Vec<&Region> {
    let mut out = Vec::new();
    for region in regions {
        out.push(region);
        if let Node::Fold { children } = &region.node {
            out.extend(all(children));
        }
    }
    out
}

fn line_count(text: &str) -> u32 {
    text.split_terminator('\n').count() as u32
}

fn assert_tiles(source: &Source) {
    let mut at = 0;
    for leaf in leaves(&source.regions) {
        assert_eq!(leaf.range.start.line, at, "gap before {leaf:?}");
        assert_eq!(leaf.range.start.column, 0);
        assert_eq!(leaf.range.end.column, 0);
        assert!(leaf.range.end.line > at, "empty leaf {leaf:?}");
        at = leaf.range.end.line;
    }
    assert_eq!(at, line_count(&source.text));
}

fn assert_folds_hold_children(regions: &[Region]) {
    for region in regions {
        if let Node::Fold { children } = &region.node {
            assert!(!children.is_empty(), "fold without children {region:?}");
            let (start, end) = region.range.lines_spanned();
            let mut at = start;
            for child in children {
                let (child_start, child_end) = child.range.lines_spanned();
                assert_eq!(child_start, at, "hole inside {region:?}");
                at = child_end;
            }
            assert_eq!(at, end, "fold {region:?} not tiled by its children");
            assert_folds_hold_children(children);
        }
    }
}

trait LinesSpanned {
    fn lines_spanned(&self) -> (u32, u32);
}

impl LinesSpanned for SourceRange {
    fn lines_spanned(&self) -> (u32, u32) {
        let range = self.lines();
        (range.start, range.end)
    }
}

const RUST_LHS: &str = "fn f(a: u32) -> u32 {\n    let x = a + 1;\n    let y = x * 2;\n    x + y\n}\n\nfn keep() -> u32 {\n    let k = 1;\n    let m = 2;\n    k + m\n}\n";
const RUST_RHS: &str = "fn f(a: u32, b: u32) -> u32 {\n    let x = a + b;\n    let y = x * 2;\n    x + y\n}\n\nfn keep() -> u32 {\n    let k = 1;\n    let m = 2;\n    k + m\n}\n\nfn added() -> u32 {\n    let p = 3;\n    let q = 4;\n    p + q\n}\n";

fn fold_ids(source: &Source) -> BTreeMap<u32, u32> {
    all(&source.regions)
        .into_iter()
        .filter(|r| matches!(r.node, Node::Fold { .. }))
        .map(|r| (r.range.start.line, r.id))
        .collect()
}

fn fold_states(source: &Source) -> BTreeMap<u32, u32> {
    all(&source.regions)
        .into_iter()
        .filter(|r| matches!(r.node, Node::Fold { .. }))
        .map(|r| (r.range.start.line, r.fold_state_id))
        .collect()
}

#[test]
fn a_closer_and_an_opener_on_one_line_yield_a_strict_tree() {
    let lhs = "fn f() {\n    for x in [\n        1,\n        2,\n    ] {\n        use_it(x);\n        more(x);\n    }\n}\n";
    let rhs = "fn f() {\n    for x in [\n        1,\n        2,\n        3,\n    ] {\n        use_it(x);\n        more(x);\n    }\n}\n";
    let diff = project("a.rs", lhs, rhs);
    let (lhs, rhs) = sources(&diff);
    for source in [lhs.unwrap(), rhs.unwrap()] {
        assert_tiles(source);
        assert_folds_hold_children(&source.regions);
        let folds: Vec<&Region> = all(&source.regions)
            .into_iter()
            .filter(|region| matches!(region.node, Node::Fold { .. }))
            .collect();
        // The array's elements open on line 2; `] {` closes it and
        // opens the loop, and belongs to neither fold.
        let collection = folds
            .iter()
            .find(|fold| fold.range.start.line == 2)
            .expect("the array is a fold");
        let body = folds
            .iter()
            .find(|fold| fold.range.start.line > collection.range.end.line)
            .expect("the loop body is a fold");
        assert!(
            collection.range.end.line < body.range.start.line,
            "the line that closes the collection and opens the body is in neither"
        );
        assert_eq!(body.range.start.column, 0);
        assert_eq!(collection.range.end.column, 0);
    }
}

#[test]
fn a_parse_error_fallback_numbers_its_folds() {
    // Both sides hold a stray `)`, so the parse-error limit of zero sends
    // the file to a line diff; the folds still come from the parse.
    let lhs = format!("{RUST_LHS})\n");
    let rhs = format!("{RUST_RHS})\n");
    let diff = project_with(
        "a.rs",
        &lhs,
        &rhs,
        DiffOptions {
            parse_error_limit: 0,
            ..DiffOptions::default()
        },
    );
    let Diff::Text { stats, .. } = &diff else {
        panic!("text diff");
    };
    assert_eq!(stats.fallback.as_ref().unwrap().code, "parse_error");
    let (lhs, rhs) = sources(&diff);
    let (lhs, rhs) = (lhs.unwrap(), rhs.unwrap());
    let (lhs_folds, rhs_folds) = (fold_ids(lhs), fold_ids(rhs));
    let lhs_ids: BTreeSet<u32> = lhs_folds.values().copied().collect();
    let rhs_ids: BTreeSet<u32> = rhs_folds.values().copied().collect();
    assert_eq!(
        lhs_ids.len(),
        lhs_folds.len(),
        "lhs folds have distinct ids"
    );
    assert_eq!(
        rhs_ids.len(),
        rhs_folds.len(),
        "rhs folds have distinct ids"
    );
    let (lhs_states, rhs_states) = (fold_states(lhs), fold_states(rhs));
    // Nothing matched the nodes these folds belong to, so no fold pairs.
    let lhs_state_ids: BTreeSet<u32> = lhs_states.values().copied().collect();
    assert!(
        !rhs_states
            .values()
            .any(|state| lhs_state_ids.contains(state)),
        "a fallback's folds are unpaired"
    );
}

#[test]
fn folds_pair_only_where_the_matcher_paired_their_nodes() {
    for options in [
        DiffOptions::default(),
        DiffOptions {
            graph_limit: 1,
            ..DiffOptions::default()
        },
    ] {
        let structural = options.graph_limit != 1;
        let diff = project_with("a.rs", RUST_LHS, RUST_RHS, options);
        let Diff::Text { stats, .. } = &diff else {
            panic!("text diff");
        };
        assert_eq!(stats.fallback.is_none(), structural);
        let (lhs, rhs) = sources(&diff);
        let (lhs, rhs) = (lhs.unwrap(), rhs.unwrap());
        assert_tiles(lhs);
        assert_tiles(rhs);
        let (lhs_folds, rhs_folds) = (fold_states(lhs), fold_states(rhs));
        assert_eq!(lhs_folds.len(), 2);
        assert_eq!(rhs_folds.len(), 3);
        let shared: BTreeSet<u32> = lhs_folds
            .values()
            .filter(|state| rhs_folds.values().any(|other| other == *state))
            .copied()
            .collect();
        if structural {
            // `f` changed its signature and stays paired through the
            // matcher; `keep` is untouched. `added` is rhs-only.
            assert_eq!(
                lhs_folds[&1], rhs_folds[&1],
                "f pairs across a changed header"
            );
            assert_eq!(lhs_folds[&7], rhs_folds[&7], "keep pairs");
            assert_eq!(shared.len(), 2);
        } else {
            // The matcher never ran, so every fold is on its own.
            assert!(shared.is_empty(), "a fallback's folds are unpaired");
        }
        assert!(
            !lhs_folds.values().any(|id| *id == rhs_folds[&13]),
            "added is rhs-only"
        );
    }
}

#[test]
fn structural_folds_pair_exactly_as_the_matcher_recorded() {
    let result = DiffResult::from_sources_with_options(
        "a.rs",
        RUST_LHS,
        RUST_RHS,
        &body_params(),
        &DiffOptions::default(),
    )
    .unwrap();
    let diff = project("a.rs", RUST_LHS, RUST_RHS);
    let (lhs, rhs) = sources(&diff);
    let lhs_folds = fold_ids(lhs.unwrap());
    let (lhs_states, rhs_states) = (fold_states(lhs.unwrap()), fold_states(rhs.unwrap()));
    let rhs_ids: BTreeSet<u32> = all(&rhs.unwrap().regions).iter().map(|r| r.id).collect();
    let rhs_states: BTreeSet<u32> = rhs_states.values().copied().collect();
    let lhs_lines: Vec<&str> = RUST_LHS.split_terminator('\n').collect();
    for fold in &result.lhs_folds {
        // A fold covers its body: its region starts after the line the
        // `{` opens on.
        let line = folds::line_span(fold, &lhs_lines).0 as u32;
        let (id, state) = (lhs_folds[&line], lhs_states[&line]);
        let matcher_paired = matches!(fold.match_kind, FoldMatch::Matched { .. });
        assert_eq!(
            rhs_states.contains(&state),
            matcher_paired,
            "{:?}",
            fold.range
        );
        // A region's `id` names it alone.
        assert!(!rhs_ids.contains(&id), "{:?}", fold.range);
    }
}

#[test]
fn a_fold_dropped_on_one_side_leaves_its_partner_unshared() {
    // The matcher pairs the two arrays, but the lhs array sits on one
    // line: it hides nothing and is not a region.
    let lhs = "fn f() {\n    let v = [1, 2];\n    work(v);\n}\n";
    let rhs = "fn f() {\n    let v = [\n        1,\n        2,\n    ];\n    work(v);\n}\n";
    let result = crate::config::diff_sources("a.rs", lhs, rhs);
    let collection = |folds: &[Fold]| -> (SyntaxId, FoldMatch) {
        // The array opens on line 1; the function body on line 0.
        let fold = folds
            .iter()
            .find(|fold| fold.range.start.line.as_usize() == 1)
            .expect("the array is a fold");
        (fold.syntax_id, fold.match_kind)
    };
    let (lhs_array, lhs_match) = collection(&result.lhs_folds);
    let (rhs_array, rhs_match) = collection(&result.rhs_folds);
    assert_eq!(
        (lhs_match, rhs_match),
        (
            FoldMatch::Matched {
                opposite: rhs_array
            },
            FoldMatch::Matched {
                opposite: lhs_array
            }
        ),
        "the matcher pairs the arrays"
    );
    let diff = project("a.rs", lhs, rhs);
    let (lhs, rhs) = sources(&diff);
    let (lhs, rhs) = (lhs.unwrap(), rhs.unwrap());
    let lhs_states: BTreeSet<u32> = all(&lhs.regions).iter().map(|r| r.fold_state_id).collect();
    let array = all(&rhs.regions)
        .into_iter()
        .find(|r| matches!(r.node, Node::Fold { .. }) && r.range.start.line == 2)
        .expect("the rhs array is a region");
    assert!(
        !lhs_states.contains(&array.fold_state_id),
        "no lhs region claims the dropped fold's state"
    );
}

#[test]
fn swapped_functions_share_fold_state_but_never_ids() {
    let lhs = "fn a() {\n    one();\n    two();\n}\n\nfn b() {\n    three();\n    four();\n}\n";
    let rhs = "fn b() {\n    three();\n    four();\n}\n\nfn a() {\n    one();\n    two();\n}\n";
    let mut result = crate::config::diff_sources("a.rs", lhs, rhs);
    // Pair each body with the body of the same function, wherever it is:
    // the pairs cross.
    let header = |src: &str, fold: &Fold| {
        src.lines()
            .nth(fold.range.start.line.as_usize())
            .unwrap()
            .to_owned()
    };
    assert_eq!(result.lhs_folds.len(), 2);
    assert_eq!(result.rhs_folds.len(), 2);
    for lhs_index in 0..2 {
        let rhs_index = result
            .rhs_folds
            .iter()
            .position(|fold| header(rhs, fold) == header(lhs, &result.lhs_folds[lhs_index]))
            .unwrap();
        result.lhs_folds[lhs_index].match_kind = FoldMatch::Matched {
            opposite: result.rhs_folds[rhs_index].syntax_id,
        };
        result.rhs_folds[rhs_index].match_kind = FoldMatch::Matched {
            opposite: result.lhs_folds[lhs_index].syntax_id,
        };
    }
    let diff = diff(
        &result,
        Inputs {
            file: &refs(true, true),
            sizes: (lhs.len() as u64, rhs.len() as u64),
            syntax: (Vec::new(), Vec::new()),
        },
    );
    let (lhs_src, rhs_src) = sources(&diff);
    let (lhs_src, rhs_src) = (lhs_src.unwrap(), rhs_src.unwrap());
    let folds = |source: &Source| -> BTreeMap<String, (u32, u32)> {
        all(&source.regions)
            .into_iter()
            .filter(|region| matches!(region.node, Node::Fold { .. }))
            .map(|region| {
                let line = region.range.start.line as usize;
                (
                    source.text.lines().nth(line).unwrap().to_owned(),
                    (region.id, region.fold_state_id),
                )
            })
            .collect()
    };
    let (lhs_folds, rhs_folds) = (folds(lhs_src), folds(rhs_src));
    assert_eq!(lhs_folds.len(), 2, "{lhs_folds:?}");
    assert_eq!(rhs_folds.len(), 2, "{rhs_folds:?}");
    for (name, &(lhs_id, lhs_state)) in &lhs_folds {
        let (_, rhs_state) = rhs_folds[name];
        assert_eq!(
            lhs_state, rhs_state,
            "{name} opens and closes on both sides"
        );
        assert!(rhs_folds.values().all(|&(id, _)| id != lhs_id));
    }
    // Leaves still tile, and every leaf pair still mirrors its splits.
    for source in [lhs_src, rhs_src] {
        assert_tiles(source);
        assert_folds_hold_children(&source.regions);
    }
    let lengths = |source: &Source| -> BTreeMap<u32, u32> {
        leaves(&source.regions)
            .into_iter()
            .map(|leaf| {
                let (start, end) = leaf.range.lines_spanned();
                (alignment(leaf), end - start)
            })
            .collect()
    };
    let rhs_lengths = lengths(rhs_src);
    for (id, length) in lengths(lhs_src) {
        if let Some(&other) = rhs_lengths.get(&id) {
            assert_eq!(length, other, "paired leaf {id}");
        }
    }
}

#[test]
fn the_fallback_keeps_folds() {
    let diff = project_with(
        "a.rs",
        RUST_LHS,
        RUST_RHS,
        DiffOptions {
            graph_limit: 1,
            ..DiffOptions::default()
        },
    );
    let Diff::Text { stats, .. } = &diff else {
        panic!("text diff");
    };
    assert_eq!(stats.fallback.as_ref().unwrap().code, "too_complex");
    let (_, rhs) = sources(&diff);
    let rhs = rhs.unwrap();
    let bodies: Vec<_> = all(&rhs.regions)
        .into_iter()
        .filter(|r| r.tags.iter().any(|tag| tag == "deleted-bodies:function"))
        .map(|r| r.range.start.line)
        .collect();
    // Nothing is collapsed before the plugins run, so the untouched `keep`
    // is a fold like the changed `f` and the new `added`.
    assert_eq!(bodies, vec![1, 7, 13]);
    assert!(leaves(&rhs.regions)
        .iter()
        .all(|leaf| !leaf.visibility.collapsed && leaf.tags.is_empty()));
}

#[test]
fn leaves_tile_both_sides_and_paired_leaves_share_ids() {
    let lhs = "import os\n\ndef f():\n    x = 1\n    return x\n";
    let rhs =
        "import os\n\ndef f():\n    x = 2\n    return x\n\ndef g():\n    y = 3\n    return y\n";
    let diff = project("a.py", lhs, rhs);
    let (lhs, rhs) = sources(&diff);
    let (lhs, rhs) = (lhs.unwrap(), rhs.unwrap());
    assert_tiles(lhs);
    assert_tiles(rhs);
    assert_folds_hold_children(&lhs.regions);
    assert_folds_hold_children(&rhs.regions);
    // Ids are dense, assigned lhs first, and never shared across sides.
    let lhs_ids: Vec<u32> = all(&lhs.regions).iter().map(|r| r.id).collect();
    let rhs_ids: Vec<u32> = all(&rhs.regions).iter().map(|r| r.id).collect();
    let mut ids: Vec<u32> = lhs_ids.iter().chain(&rhs_ids).copied().collect();
    ids.sort_unstable();
    assert_eq!(ids, (1..=ids.len() as u32).collect::<Vec<_>>());
    assert!(lhs_ids.iter().max() < rhs_ids.iter().min());
    // Leaf alignment ids are dense on their own counter.
    let lhs_leaves: BTreeMap<u32, &Region> = leaves(&lhs.regions)
        .into_iter()
        .map(|leaf| (alignment(leaf), leaf))
        .collect();
    let rhs_leaves: BTreeMap<u32, &Region> = leaves(&rhs.regions)
        .into_iter()
        .map(|leaf| (alignment(leaf), leaf))
        .collect();
    let alignments: BTreeSet<u32> = lhs_leaves
        .keys()
        .chain(rhs_leaves.keys())
        .copied()
        .collect();
    assert_eq!(
        alignments.into_iter().collect::<Vec<_>>(),
        (0..=*lhs_leaves.keys().chain(rhs_leaves.keys()).max().unwrap()).collect::<Vec<_>>()
    );
    let mut paired = 0;
    for (id, lhs_leaf) in &lhs_leaves {
        if let Some(rhs_leaf) = rhs_leaves.get(id) {
            paired += 1;
            assert_eq!(
                lhs_leaf.range.lines_spanned().1 - lhs_leaf.range.lines_spanned().0,
                rhs_leaf.range.lines_spanned().1 - rhs_leaf.range.lines_spanned().0,
                "paired leaves have equal length"
            );
            assert_eq!(lhs_leaf.fold_state_id, rhs_leaf.fold_state_id);
        }
    }
    assert!(paired > 0);
    // Python's body fold covers the body alone: it starts on the line
    // after the `def` line, which the leaf before it holds.
    let new_fold = all(&rhs.regions)
        .into_iter()
        .find(|r| matches!(r.node, Node::Fold { .. }) && r.range.start.line == 7)
        .expect("the added function is a fold");
    // The new function exists on the rhs only: nothing on the lhs opens
    // with it or aligns with its rows.
    let lhs_states: BTreeSet<u32> = all(&lhs.regions).iter().map(|r| r.fold_state_id).collect();
    assert!(!lhs_states.contains(&new_fold.fold_state_id));
    assert!(leaves(std::slice::from_ref(new_fold))
        .into_iter()
        .all(|leaf| !lhs_leaves.contains_key(&alignment(leaf))));
    assert_eq!(
        new_fold.tags,
        vec![
            "deleted-bodies:function",
            "removed-runs:function",
            "summarize:function"
        ]
    );
    // A label is a plugin's to give; the projection leaves it empty.
    assert_eq!(new_fold.visibility.label, "");
}

#[test]
fn the_changed_body_is_a_paired_fold_with_a_novel_leaf_inside() {
    let lhs = "def f():\n    a = 1\n    b = 2\n    return a\n";
    let rhs = "def f():\n    a = 1\n    b = 3\n    return a\n";
    let diff = project("a.py", lhs, rhs);
    let (lhs, rhs) = sources(&diff);
    let (lhs, rhs) = (lhs.unwrap(), rhs.unwrap());
    let fold = |source: &Source| {
        let folds: Vec<_> = all(&source.regions)
            .into_iter()
            .filter(|r| matches!(r.node, Node::Fold { .. }))
            .collect();
        assert_eq!(folds.len(), 1);
        folds[0].clone()
    };
    let lhs_fold = fold(lhs);
    let rhs_fold = fold(rhs);
    assert_ne!(lhs_fold.id, rhs_fold.id);
    assert_eq!(lhs_fold.fold_state_id, rhs_fold.fold_state_id);
    assert_eq!(lhs_fold.range.lines_spanned(), (1, 4));
    let novel: Vec<_> = leaves(&rhs.regions)
        .into_iter()
        .filter(|leaf| matches!(&leaf.node, Node::Leaf { changed, .. } if !changed.is_empty()))
        .collect();
    assert_eq!(novel.len(), 1);
    assert_eq!(novel[0].range.lines_spanned(), (2, 3));
    let Node::Leaf { changed, .. } = &novel[0].node else {
        unreachable!()
    };
    // Only the changed token is painted, not the whole line.
    assert_eq!(
        changed,
        &[Span {
            line: 2,
            start_column: 8,
            end_column: 9
        }]
    );
}

#[test]
fn python_body_folds_pair_through_their_block_nodes() {
    // The body fold is the `block` under `def`, opening after the
    // header's `:`. The header changed too; the blocks still pair by
    // node identity, and their folds share one `fold_state_id`.
    let lhs = "def f(a):\n    x = a\n    y = 2\n    return x + y\n";
    let rhs = "def f(a, b):\n    x = a\n    y = 3\n    return x + y\n";
    let result = crate::config::diff_sources("a.py", lhs, rhs);
    assert_eq!(result.lhs_folds.len(), 1);
    assert_eq!(result.rhs_folds.len(), 1);
    let start = |fold: &Fold| {
        (
            fold.range.start.line.as_usize(),
            fold.range.start.byte_column,
        )
    };
    assert_eq!(start(&result.lhs_folds[0]), (0, 9));
    assert_eq!(start(&result.rhs_folds[0]), (0, 12));
    assert_eq!(
        result.lhs_folds[0].match_kind,
        FoldMatch::Matched {
            opposite: result.rhs_folds[0].syntax_id
        }
    );
    assert_eq!(
        result.rhs_folds[0].match_kind,
        FoldMatch::Matched {
            opposite: result.lhs_folds[0].syntax_id
        }
    );
    let diff = project("a.py", lhs, rhs);
    let (lhs, rhs) = sources(&diff);
    let fold = |source: &Source| {
        all(&source.regions)
            .into_iter()
            .find(|r| matches!(r.node, Node::Fold { .. }))
            .expect("the body is a fold")
            .clone()
    };
    let (lhs_fold, rhs_fold) = (fold(lhs.unwrap()), fold(rhs.unwrap()));
    assert_eq!(lhs_fold.fold_state_id, rhs_fold.fold_state_id);
    assert_ne!(lhs_fold.id, rhs_fold.id);
}

#[test]
fn a_fully_new_line_is_painted_whole_and_blank_lines_not_at_all() {
    let diff = project("a.py", "x = 1\n", "x = 1\n\ny = 2\n");
    let (_, rhs) = sources(&diff);
    let changed: Vec<Span> = leaves(&rhs.unwrap().regions)
        .into_iter()
        .filter_map(|leaf| match &leaf.node {
            Node::Leaf { changed, .. } => Some(changed.clone()),
            Node::Fold { .. } => None,
        })
        .flatten()
        .collect();
    assert_eq!(
        changed,
        vec![Span {
            line: 2,
            start_column: 0,
            end_column: 5
        }]
    );
}

#[test]
fn a_fold_edge_splits_paired_leaves_on_both_sides() {
    // The unchanged function forces a split on both sides at the same
    // offset, even though only the rhs shifted.
    let lhs = "a = 1\n\ndef f():\n    return 1\n\nz = 1\n";
    let rhs = "a = 2\n\ndef f():\n    return 1\n\nz = 1\n";
    let diff = project("a.py", lhs, rhs);
    let (lhs, rhs) = sources(&diff);
    let (lhs, rhs) = (lhs.unwrap(), rhs.unwrap());
    assert_tiles(lhs);
    assert_tiles(rhs);
    assert_folds_hold_children(&lhs.regions);
    assert_folds_hold_children(&rhs.regions);
    let lhs_leaves: Vec<_> = leaves(&lhs.regions)
        .iter()
        .map(|l| (alignment(l), l.range.lines_spanned()))
        .collect();
    let rhs_leaves: Vec<_> = leaves(&rhs.regions)
        .iter()
        .map(|l| (alignment(l), l.range.lines_spanned()))
        .collect();
    assert_eq!(lhs_leaves, rhs_leaves);
}

#[test]
fn stats_count_changed_lines_and_flag_unsupported_languages() {
    let diff = project("a.py", "x = 1\n", "x = 1 # same\n");
    let Diff::Text { stats, .. } = &diff else {
        panic!("text")
    };
    assert_eq!(
        stats.textual,
        LineCounts {
            added: 1,
            removed: 1
        }
    );
    assert!(stats.fallback.is_none());
    let diff = project("a.unknownext", "x\n", "y\n");
    let Diff::Text { stats, .. } = &diff else {
        panic!("text")
    };
    assert_eq!(
        stats.fallback.as_ref().unwrap().code,
        "unsupported_language"
    );
}

#[test]
fn one_sided_files_have_one_source_and_no_shared_ids() {
    let diff = project("a.py", "", "def f():\n    return 1\n");
    let (lhs, rhs) = sources(&diff);
    assert!(lhs.is_none());
    let rhs = rhs.unwrap();
    assert_tiles(rhs);
    assert!(leaves(&rhs.regions)
        .iter()
        .all(|leaf| !leaf.visibility.collapsed));
}

#[test]
fn binary_sides_carry_sizes() {
    let result = DiffResult {
        file_format: FileFormat::Binary,
        lhs_src: FileContent::Binary,
        rhs_src: FileContent::Binary,
        lhs_folds: vec![],
        rhs_folds: vec![],
        lhs_positions: vec![],
        rhs_positions: vec![],
    };
    let diff = diff(
        &result,
        Inputs {
            file: &refs(true, true),
            sizes: (3, 5),
            syntax: (Vec::new(), Vec::new()),
        },
    );
    let Diff::Binary {
        sides: Pairing::Both { lhs, rhs },
    } = diff
    else {
        panic!("a binary diff with both sides: {diff:?}");
    };
    assert_eq!((lhs.size, rhs.size), (3, 5));
}

#[test]
fn syntax_spans_are_per_line_sorted_and_innermost() {
    let parser = crate::parse::tree_sitter_parser::from_language(
        crate::parse::guess_language::Language::Python,
    );
    let spans = syntax_spans("def f(x):\n    return \"a\"\n", parser);
    for pair in spans.windows(2) {
        assert!(
            pair[0].line < pair[1].line
                || (pair[0].line == pair[1].line && pair[0].end_column <= pair[1].start_column),
            "{pair:?}"
        );
    }
    assert!(spans
        .iter()
        .any(|span| span.capture == "keyword" && span.line == 0));
    assert!(spans
        .iter()
        .any(|span| span.capture.starts_with("string") && span.line == 1));
}
