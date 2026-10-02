use super::*;

const TEST: &str = "test-bodies:test";

/// Every fold: whether it is a test body, whether it starts collapsed, and
/// its label.
fn folds(source: &tree::Source) -> Vec<(bool, bool, String)> {
    let mut folds = Vec::new();
    walk(&source.regions, &mut |region| {
        if is_fold(region) {
            folds.push((
                has_tag(region, TEST),
                region.visibility.collapsed,
                region.visibility.label.clone(),
            ));
        }
    });
    folds
}

#[test]
fn test_bodies_collapse_on_both_sides_and_stay_expandable() {
    let before = "#[test]\nfn t() {\n    a();\n    b();\n    c();\n}\n\nfn f() {\n    a();\n    b();\n    c();\n}\n";
    let after = "#[test]\nfn t() {\n    a();\n    b();\n    changed();\n}\n\nfn f() {\n    a();\n    b();\n    c();\n}\n";
    let (file, mut sides) = project("a.rs", before, after);
    run("test-bodies", json!({"min_lines": 3}), &file, &mut sides);
    let sides = trees(&sides);
    let tree::Pairing::Both {
        lhs: before,
        rhs: after,
    } = &sides
    else {
        panic!("both sides");
    };
    for source in [before, after] {
        assert_eq!(
            folds(source),
            vec![
                // The context queries wrap each function in a scope fold,
                // which the body fold nests inside.
                (false, false, String::new()),
                (true, true, "test body".to_owned()),
                (false, false, String::new()),
                // No plugin collapsed `f`'s body, so it has no label.
                (false, false, String::new())
            ]
        );
    }
    // A `#[cfg(test)]` module collapses as one labelled fold; the test
    // bodies inside keep their own label.
    let (file, mut sides) = project(
        "a.rs",
        "",
        "#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        a();\n        b();\n        c();\n    }\n}\n",
    );
    run("test-bodies", json!({"min_lines": 3}), &file, &mut sides);
    let sides = trees(&sides);
    let mut labels = Vec::new();
    walk(&rhs(&sides).regions, &mut |region| {
        if is_fold(region) && region.visibility.collapsed {
            labels.push(region.visibility.label.clone());
        }
    });
    assert_eq!(labels, ["test module", "test body"]);
    // A tiny test body stays open.
    let (file, mut sides) = project("a.rs", "", "#[test]\nfn t() {\n    a();\n}\n");
    run("test-bodies", json!({"min_lines": 3}), &file, &mut sides);
    let sides = trees(&sides);
    walk(&rhs(&sides).regions, &mut |region| {
        assert!(!region.visibility.collapsed);
    });
}

#[test]
fn a_test_body_links_its_docstring_on_each_side() {
    let before = "/// Stays.\n/// Here.\n#[test]\nfn t() {\n    a();\n    b();\n    c();\n}\n";
    let after = "/// Stays.\n/// Here.\n#[test]\nfn t() {\n    a();\n    b();\n    d();\n}\n";
    let (file, mut sides) = project("a.rs", before, after);
    run("test-bodies", json!({"min_lines": 3}), &file, &mut sides);
    let sides = trees(&sides);
    let tree::Pairing::Both { lhs, rhs } = &sides else {
        panic!("both sides");
    };
    let mut states = Vec::new();
    for source in [lhs, rhs] {
        walk(&source.regions, &mut |region| {
            if has_tag(region, TEST) || has_tag(region, "test-bodies:docstring") {
                assert!(region.visibility.collapsed);
                states.push(region.fold_state_id);
            }
        });
    }
    assert_eq!(states.len(), 4, "{states:?}");
    assert!(
        states.iter().all(|state| *state == states[0]),
        "the matched docstrings and bodies share one fold state: {states:?}"
    );
}

#[test]
fn javascript_test_callbacks_collapse() {
    for path in ["a.test.ts", "a.test.js", "a.test.tsx"] {
        let (file, mut sides) = project(
            path,
            "",
            "it('adds', () => {\n  expect(1).toBe(1);\n  expect(2).toBe(2);\n});\n\nrun('z', () => {\n  go();\n  go();\n});\n",
        );
        run("test-bodies", json!({"min_lines": 2}), &file, &mut sides);
        let sides = trees(&sides);
        let mut collapsed = Vec::new();
        walk(&rhs(&sides).regions, &mut |region| {
            if region.visibility.collapsed {
                collapsed.push(region.range.start.line);
            }
        });
        assert_eq!(collapsed, [1], "{path}");
    }
}
