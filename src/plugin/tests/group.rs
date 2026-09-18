use super::*;
use diffr_plugin_sdk::tree::line_count;
use std::collections::BTreeSet;

fn range(start: u32, end: u32) -> types::Range {
    types::Range {
        start: types::Position {
            line: start,
            column: 0,
        },
        end: types::Position {
            line: end,
            column: 0,
        },
    }
}

fn leaf(id: u32, alignment: u32, start: u32, end: u32) -> tree::Region {
    tree::Region {
        id,
        fold_state_id: id,
        range: range(start, end),
        tags: vec![],
        visibility: types::Visibility::default(),
        node: tree::Node::Leaf {
            alignment_id: alignment,
            search_highlights: Vec::new(),
            changed: vec![],
        },
    }
}

/// A fold over one leaf, collapsed or not, in fold state `state`.
fn fold(id: u32, state: u32, start: u32, end: u32, collapsed: bool, label: &str) -> tree::Region {
    tree::Region {
        id,
        fold_state_id: state,
        range: range(start, end),
        tags: vec![],
        visibility: types::Visibility {
            collapsed,
            label: label.to_owned(),
        },
        node: tree::Node::Fold {
            children: vec![leaf(id + 100, id + 100, start, end)],
        },
    }
}

fn source(regions: Vec<tree::Region>) -> tree::Source {
    tree::Source {
        text: String::new(),
        regions,
    }
}

#[test]
fn collapsed_regions_group_across_short_separators_whatever_their_labels() {
    let mut sides = tree::Pairing::LeftOnly {
        lhs: source(vec![
            fold(1, 1, 0, 5, true, "5 lines removed"),
            leaf(2, 2, 5, 7),
            fold(3, 3, 7, 12, true, "5 unchanged lines"),
            leaf(4, 4, 12, 15),
            fold(5, 5, 15, 20, true, "test body"),
        ]),
    };
    let (file, _) = project("m.py", "", "");
    run_trees("group", json!({}), &file, &mut sides);
    let regions = &lhs(&sides).regions;
    assert_eq!(
        regions.len(),
        3,
        "three open lines end the run: {regions:?}"
    );
    assert_eq!(
        regions[0].visibility.label,
        "2 collapsed regions · 12 lines"
    );
    assert!(regions[0].visibility.collapsed);
    let tree::Node::Fold { children } = &regions[0].node else {
        panic!("a group is a fold");
    };
    let ids: Vec<u32> = children.iter().map(|child| child.id).collect();
    assert_eq!(ids, [1, 2, 3]);
}

#[test]
fn deleted_bodies_under_their_own_headers_are_not_grouped() {
    // Each deleted body collapses inside the scope of its `def`, which
    // shows that `def` line above the collapsed row. A group never hides
    // open lines above its first collapsed row, so the run never starts and
    // the reader keeps the name of every function that went.
    let before = "def a():\n    x()\n    y()\n    z()\n\ndef b():\n    x()\n    y()\n    z()\n\ndef c():\n    x()\n    y()\n    z()\n\nkeep = 1\n";
    let after = "keep = 1\n";
    let (file, mut sides) = project("m.py", before, after);
    run("deleted-bodies", json!({"min_lines": 3}), &file, &mut sides);
    run("group", json!({}), &file, &mut sides);
    let sides = trees(&sides);
    assert!(
        !lhs(&sides)
            .regions
            .iter()
            .any(|region| region.visibility.label.contains("collapsed regions")),
        "no group hides the `def` lines"
    );
    let mut bodies = Vec::new();
    let mut headers = Vec::new();
    walk(&lhs(&sides).regions, &mut |region| {
        if region.visibility.collapsed {
            bodies.push(region.range.lines());
        }
        if has_tag(region, "context:scope") {
            headers.push(region.range.start.line);
        }
    });
    assert_eq!(bodies.len(), 3, "{bodies:?}");
    assert_eq!(headers, [0, 5, 10]);
    // Each collapsed body starts under the `def` line of its own scope.
    for (body, header) in bodies.iter().zip(&headers) {
        assert_eq!(body.start, header + 1);
    }
    let mut seen = BTreeSet::new();
    walk(&lhs(&sides).regions, &mut |region| {
        assert!(seen.insert(region.id), "duplicate id {}", region.id);
    });
}

#[test]
fn documented_test_bodies_group_with_their_docstrings() {
    let before = "fn keep() {}\n";
    let after = "fn keep() {}\n\n/// First.\n/// Doc.\n#[test]\nfn a() {\n    x();\n    y();\n    z();\n}\n\n/// Second.\n/// Doc.\n#[test]\nfn b() {\n    x();\n    y();\n    z();\n}\n\n/// Third.\n/// Doc.\n#[test]\nfn c() {\n    x();\n    y();\n    z();\n}\n";
    let (file, mut sides) = project("m.rs", before, after);
    run("test-bodies", json!({"min_lines": 3}), &file, &mut sides);
    run("group", json!({}), &file, &mut sides);
    let sides = trees(&sides);
    let groups: Vec<&tree::Region> = rhs(&sides)
        .regions
        .iter()
        .filter(|region| region.visibility.label.contains("collapsed regions"))
        .collect();
    assert_eq!(groups.len(), 1, "one group for the three documented tests");
    let group = groups[0];
    let tree::Node::Fold { children } = &group.node else {
        panic!("a group is a fold");
    };
    let mut collapsed = Vec::new();
    walk(children, &mut |region| {
        if region.visibility.collapsed {
            collapsed.push(region.range);
        }
    });
    assert_eq!(
        collapsed.len(),
        6,
        "every body and docstring is inside the group"
    );
    assert_eq!(
        group.range.start, collapsed[0].start,
        "the group starts at the first docstring"
    );
    assert_eq!(
        group.visibility.label,
        format!(
            "6 collapsed regions · {} lines",
            children.iter().map(line_count).sum::<usize>()
        )
    );
}

#[test]
fn matched_runs_are_grouped_on_both_sides_with_one_fold_state() {
    // Two collapsed test bodies moved below `keep`: each matched pair
    // shares its fold state, so the runs match region for region.
    let mut sides = tree::Pairing::Both {
        lhs: source(vec![
            fold(1, 1, 0, 4, true, "test body"),
            fold(2, 2, 4, 8, true, "test body"),
            leaf(3, 3, 8, 11),
        ]),
        rhs: source(vec![
            leaf(6, 3, 0, 3),
            fold(4, 1, 3, 7, true, "test body"),
            fold(5, 2, 7, 11, true, "test body"),
        ]),
    };
    let (file, _) = project("m.py", "", "");
    let joins: Vec<Move> = moves(&bundled("group", json!({})), &file, &wire(sides.clone()))
        .unwrap()
        .into_iter()
        .filter(|next| matches!(next, Move::JoinFolds(_)))
        .collect();
    assert_eq!(joins.len(), 1, "one join lists both runs: {joins:?}");
    run_trees("group", json!({}), &file, &mut sides);
    assert_eq!(lhs(&sides).regions.len(), 2);
    assert_eq!(rhs(&sides).regions.len(), 2);
    let (lhs_group, rhs_group) = (&lhs(&sides).regions[0], &rhs(&sides).regions[1]);
    assert_eq!(lhs_group.visibility.label, "2 collapsed regions · 8 lines");
    assert_eq!(rhs_group.visibility.label, "2 collapsed regions · 8 lines");
    assert_ne!(lhs_group.id, rhs_group.id);
    assert_eq!(lhs_group.fold_state_id, rhs_group.fold_state_id);
}

#[test]
fn a_run_paired_with_a_different_run_is_left_alone() {
    // The lhs run's second body is matched with a fold outside any run
    // on the rhs.
    let sides = tree::Pairing::Both {
        lhs: source(vec![
            fold(1, 1, 0, 4, true, "test body"),
            fold(2, 2, 4, 8, true, "test body"),
        ]),
        rhs: source(vec![
            fold(4, 1, 0, 4, true, "test body"),
            fold(5, 2, 4, 8, false, "test body"),
        ]),
    };
    let (file, _) = project("m.py", "", "");
    assert!(moves(&bundled("group", json!({})), &file, &wire(sides))
        .unwrap()
        .is_empty());
}

#[test]
fn a_single_collapsed_fold_is_left_alone() {
    let before = "def a():\n    x()\n    y()\n    z()\n\nkeep = 1\n";
    let after = "keep = 1\n";
    let (file, mut sides) = project("m.py", before, after);
    run("deleted-bodies", json!({"min_lines": 3}), &file, &mut sides);
    assert!(moves(&bundled("group", json!({})), &file, &sides)
        .unwrap()
        .is_empty());
}
