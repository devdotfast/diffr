use super::*;
use std::collections::BTreeSet;

/// One rendered line per value, concatenated.
fn repeated(values: impl Iterator<Item = u32>, render: impl Fn(u32) -> String) -> String {
    let mut out = String::new();
    for value in values {
        out.push_str(&render(value));
    }
    out
}

fn shaped(path: &str, before: &str, after: &str, lines: u32) -> tree::Pairing<tree::Source> {
    let (file, mut sides) = project(path, before, after);
    run("context", json!({ "lines": lines }), &file, &mut sides);
    trees(&sides)
}

/// Every region's lines and whether it starts collapsed, with its label,
/// in document order.
fn rows(regions: &[tree::Region]) -> Vec<((u32, u32), bool, String, bool)> {
    let mut out = Vec::new();
    walk(regions, &mut |region| {
        out.push((
            (region.range.start.line, region.range.end.line),
            region.visibility.collapsed,
            region.visibility.label.clone(),
            is_fold(region),
        ))
    });
    out
}

/// Lines no collapsed region, or region under one, hides.
fn open_lines(regions: &[tree::Region]) -> BTreeSet<u32> {
    fn visit(regions: &[tree::Region], out: &mut BTreeSet<u32>) {
        for region in regions {
            if region.visibility.collapsed {
                // The header of a collapsed fold stays visible.
                if is_fold(region) {
                    out.insert(region.range.start.line);
                }
                continue;
            }
            match &region.node {
                tree::Node::Leaf { .. } => {
                    out.extend(region.range.start.line..region.range.end.line)
                }
                tree::Node::Fold { children } => visit(children, out),
            }
        }
    }
    let mut out = BTreeSet::new();
    visit(regions, &mut out);
    out
}

#[test]
fn long_unchanged_runs_collapse_to_the_context_width() {
    let body = repeated(0..20, |i| format!("x{i} = {i}\n"));
    let before = format!("{body}changed = 1\n{body}");
    let after = format!("{body}changed = 2\n{body}");
    let sides = shaped("a.py", &before, &after, 3);
    let leaves: Vec<_> = rows(&lhs(&sides).regions)
        .into_iter()
        .map(|(lines, collapsed, label, _)| (lines, collapsed, label))
        .collect();
    assert_eq!(
        leaves,
        vec![
            ((0, 17), true, "17 unchanged lines".to_owned()),
            ((17, 20), false, String::new()),
            ((20, 21), false, String::new()),
            ((21, 24), false, String::new()),
            ((24, 41), true, "17 unchanged lines".to_owned()),
        ]
    );
    let sides = shaped("a.py", &before, &after, 1);
    assert_eq!(
        rows(&rhs(&sides).regions)[0],
        ((0, 19), true, "19 unchanged lines".to_owned(), false)
    );
}

#[test]
fn a_stretch_over_whole_folds_collapses_as_one_group() {
    // Three unchanged functions sit between two changes: their folds,
    // the lines between them and the cut ends collapse under one row.
    let unchanged = repeated(0..3, |i| {
        format!("def f{i}():\n    a = {i}\n    b = {i}\n    return a + b\n\n")
    });
    let before = format!("first = 1\n\n{unchanged}last = 1\n");
    let after = format!("first = 2\n\n{unchanged}last = 2\n");
    let sides = shaped("a.py", &before, &after, 1);
    for source in [lhs(&sides), rhs(&sides)] {
        let top: Vec<_> = source
            .regions
            .iter()
            .filter(|region| region.visibility.collapsed)
            .map(|region| {
                (
                    (region.range.start.line, region.range.end.line),
                    region.visibility.label.clone(),
                    is_fold(region),
                )
            })
            .collect();
        assert_eq!(top, [((2, 16), "14 unchanged lines".to_owned(), true)]);
    }
    let tree::Pairing::Both { lhs, rhs } = &sides else {
        panic!("both sides");
    };
    fn group(source: &tree::Source) -> Option<&tree::Region> {
        source
            .regions
            .iter()
            .find(|region| region.visibility.collapsed)
    }
    let (lhs_group, rhs_group) = (group(lhs).unwrap(), group(rhs).unwrap());
    assert_ne!(lhs_group.id, rhs_group.id);
    assert_eq!(
        lhs_group.fold_state_id, rhs_group.fold_state_id,
        "the two sides' groups open and close together"
    );
    assert_eq!(open_lines(&lhs.regions), BTreeSet::from([0, 1, 2, 16, 17]));
}

#[test]
fn slivers_cut_from_a_stretch_by_a_fold_edge_stay_open() {
    // The changed function's fold edge lands inside the stretch above the
    // change; the two lines left inside the fold are not worth a row.
    let head = repeated(0..8, |i| format!("x{i} = {i}\n"));
    let before = format!("{head}\ndef f():\n    a = 1\n    b = 1\n    c = 1\n    return 1\n");
    let after = format!("{head}\ndef f():\n    a = 1\n    b = 1\n    c = 1\n    return 2\n");
    let sides = shaped("a.py", &before, &after, 1);
    let collapsed: Vec<_> = rows(&rhs(&sides).regions)
        .into_iter()
        .filter(|(_, collapsed, ..)| *collapsed)
        .map(|(lines, ..)| lines)
        .collect();
    assert_eq!(collapsed, [(0, 9)]);
}

#[test]
fn the_enclosing_header_stays_open_above_a_deep_change() {
    // A change ten lines into a function whose signature runs to three
    // lines, under ten unchanged lines of its own. The scope is the whole
    // `fn`, so the line it starts on is shown even though it is far outside
    // the padding and its body fold opens two lines below it.
    let head = repeated(0..10, |i| format!("const C{i}: u32 = {i};\n"));
    let body = repeated(0..10, |i| format!("    let a{i} = {i};\n"));
    let signature = "fn outer(\n    a: u32,\n    b: u32,\n    c: u32,\n) -> u32 {\n";
    let before = format!("{head}{signature}{body}    1\n}}\n");
    let after = format!("{head}{signature}{body}    2\n}}\n");
    let sides = shaped("a.rs", &before, &after, 1);
    let open = open_lines(&rhs(&sides).regions);
    assert_eq!(open, BTreeSet::from([10, 24, 25, 26]));
    // Without a context query the whole signature collapses with the
    // unchanged lines above it.
    let config = Config::default();
    let queries = Pipeline::from_config(&config.plugins, std::path::Path::new("."))
        .unwrap()
        .queries()
        .unwrap()
        .into_iter()
        .filter(|(plugin, _)| plugin != "context")
        .collect();
    let params = config.compile_queries(queries).unwrap();
    let (file, mut sides) =
        project_compiled("a.rs", &before, &after, &params, DiffOptions::default());
    run("context", json!({"lines": 1}), &file, &mut sides);
    let sides = trees(&sides);
    assert_eq!(
        open_lines(&rhs(&sides).regions),
        BTreeSet::from([24, 25, 26])
    );
}

#[test]
fn a_file_the_diff_does_not_parse_has_no_enclosing_header() {
    // A generated file is diffed by line without parsing, so its
    // context query never runs.
    let body = repeated(0..10, |i| format!("    let a{i} = {i};\n"));
    let before = format!("fn outer() -> u32 {{\n{body}    1\n}}\n");
    let after = format!("fn outer() -> u32 {{\n{body}    2\n}}\n");
    let (file, mut sides) = project_with(
        "a.rs",
        &before,
        &after,
        DiffOptions {
            generated: true,
            ..DiffOptions::default()
        },
    );
    let mut scopes = 0;
    walk(&rhs(&trees(&sides)).regions, &mut |region| {
        scopes += usize::from(has_tag(region, "context:scope"));
    });
    assert_eq!(scopes, 0);
    run("context", json!({"lines": 1}), &file, &mut sides);
    let sides = trees(&sides);
    assert_eq!(
        open_lines(&rhs(&sides).regions),
        BTreeSet::from([10, 11, 12])
    );
}

#[test]
fn a_stretch_crossing_a_fold_end_collapses_on_each_side_of_it() {
    // The inner block's closer sits inside a long unchanged stretch that
    // continues in the enclosing function. Regions are not refitted
    // around it, so the part inside the block and the part after it
    // collapse separately.
    let body = repeated(1..=7, |n| format!("        u{n}();\n"));
    let tail = repeated(1..=5, |n| format!("    v{n}();\n"));
    let before = format!("fn f() {{\n    if a {{\n        x();\n{body}    }}\n{tail}}}\n");
    let after = format!("fn f() {{\n    if a {{\n        y();\n{body}    }}\n{tail}}}\n");
    let sides = shaped("a.rs", &before, &after, 1);
    for source in [lhs(&sides), rhs(&sides)] {
        let collapsed: Vec<_> = rows(&source.regions)
            .into_iter()
            .filter(|(_, collapsed, ..)| *collapsed)
            .map(|(lines, _, label, _)| (lines, label))
            .collect();
        assert_eq!(
            collapsed,
            [
                ((4, 10), "6 unchanged lines".to_owned()),
                ((10, 16), "6 unchanged lines".to_owned())
            ]
        );
        // The inner block ends before the line its `}` sits on, and the
        // function's scope runs to the brace that closes it: the scope keeps
        // the line it opens on and the line it closes on.
        let open = open_lines(&source.regions);
        assert!(open.contains(&0) && open.contains(&16), "{open:?}");
    }
}

#[test]
fn identical_files_collapse_whole_and_one_sided_files_stay_open() {
    let sides = shaped("a.py", "x = 1\ny = 2\n", "x = 1\ny = 2\n", 3);
    assert_eq!(
        rows(&lhs(&sides).regions),
        [((0, 2), true, "2 unchanged lines".to_owned(), false)]
    );
    assert_eq!(
        lhs(&sides).regions[0].alignment_id(),
        rhs(&sides).regions[0].alignment_id()
    );
    let (file, sides) = project("a.py", "", "def f():\n    return 1\n");
    let tree::Pairing::Both { rhs: after, .. } = trees(&sides) else {
        panic!("both sides");
    };
    let mut sides = tree::Pairing::RightOnly { rhs: after };
    run_trees("context", json!({"lines": 3}), &file, &mut sides);
    assert!(rows(&rhs(&sides).regions)
        .iter()
        .all(|(_, collapsed, ..)| !collapsed));
}

#[test]
fn a_line_diff_fallback_has_unpaired_folds() {
    const BEFORE: &str = "fn f(a: u32) -> u32 {\n    let x = a + 1;\n    let y = x * 2;\n    let z = y * 2;\n    let w = z * 2;\n    x + y\n}\n\nfn keep() -> u32 {\n    let k = 1;\n    let m = 2;\n    k + m\n}\n";
    const AFTER: &str = "fn f(a: u32) -> u32 {\n    let x = a + 1;\n    let y = x * 2;\n    let z = y * 2;\n    let w = z * 2;\n    x + y + 1\n}\n\nfn keep() -> u32 {\n    let k = 1;\n    let m = 2;\n    k + m\n}\n";
    let (file, mut sides) = project_with(
        "a.rs",
        BEFORE,
        AFTER,
        DiffOptions {
            graph_limit: 1,
            ..DiffOptions::default()
        },
    );
    run("context", json!({"lines": 1}), &file, &mut sides);
    let sides = trees(&sides);
    let open = open_lines(&rhs(&sides).regions);
    // The parse's folds stand, so the changed function's header does.
    assert!(open.contains(&0), "{open:?}");
    assert!(!open.contains(&2), "{open:?}");
    // Nothing matched the nodes the folds belong to, so the two sides' folds
    // are unpaired: the unchanged function below collapses on the side the
    // stretch is shaped from, and stays open on the other.
    let tree::Pairing::Both { lhs: before, .. } = &sides else {
        panic!("both sides");
    };
    assert!(!open_lines(&before.regions).contains(&10), "{open:?}");
    assert!(open.contains(&10), "{open:?}");
}

#[test]
fn a_fold_whose_matched_partner_holds_changes_stays_open() {
    // Fold 1 on the lhs lies inside an unchanged stretch, but its
    // matched partner on the rhs (fold 5, sharing the fold state) moved
    // below and holds new lines. Collapsing fold 1 would hide it.
    let range = |start: u32, end: u32| types::Range {
        start: types::Position {
            line: start,
            column: 0,
        },
        end: types::Position {
            line: end,
            column: 0,
        },
    };
    let leaf = |id: u32, alignment: u32, start: u32, end: u32, changed: bool| tree::Region {
        id,
        fold_state_id: id,
        range: range(start, end),
        tags: vec![],
        visibility: types::Visibility::default(),
        node: tree::Node::Leaf {
            alignment_id: alignment,
            search_highlights: Vec::new(),
            changed: (start..end)
                .filter(|_| changed)
                .map(|line| types::Span {
                    line,
                    start_column: 0,
                    end_column: 1,
                })
                .collect(),
        },
    };
    // The rhs leaf paired with the lhs leaf `lhs`, whose id is also its
    // alignment id.
    let paired = |id: u32, lhs: u32, start: u32, end: u32, changed: bool| tree::Region {
        fold_state_id: lhs,
        ..leaf(id, lhs, start, end, changed)
    };
    let fold = |id: u32, state: u32, child: tree::Region| tree::Region {
        id,
        fold_state_id: state,
        range: child.range,
        tags: vec![],
        visibility: types::Visibility::default(),
        node: tree::Node::Fold {
            children: vec![child],
        },
    };
    let source = |lines: usize, regions| tree::Source {
        text: "x\n".repeat(lines),
        regions,
    };
    let mut sides = tree::Pairing::Both {
        lhs: source(
            6,
            vec![
                fold(1, 1, leaf(2, 2, 0, 4, false)),
                leaf(7, 7, 4, 5, false),
                leaf(3, 3, 5, 6, true),
            ],
        ),
        rhs: source(
            10,
            vec![
                paired(8, 2, 0, 4, false),
                paired(9, 7, 4, 5, false),
                paired(10, 3, 5, 6, true),
                fold(5, 1, leaf(6, 6, 6, 10, true)),
            ],
        ),
    };
    let (file, _) = project("a.py", "", "");
    run_trees("context", json!({"lines": 1}), &file, &mut sides);
    let collapsed = |source: &tree::Source| {
        rows(&source.regions)
            .into_iter()
            .filter(|(_, collapsed, _, fold)| *collapsed && *fold)
            .count()
    };
    assert_eq!(collapsed(lhs(&sides)), 0);
    assert_eq!(collapsed(rhs(&sides)), 0);
}

#[test]
fn predicted_group_ids_match_the_applier() {
    // The stretch starts inside one leaf and ends inside another, with
    // folds between: the group names the pieces the cuts before it made,
    // and the applier accepts it on both sides.
    let block = repeated(0..8, |i| format!("x{i} = {i}\n"));
    let functions = repeated(0..2, |i| {
        format!("def g{i}():\n    a = {i}\n    b = {i}\n\n")
    });
    let before = format!("changed = 1\n{block}{functions}{block}changed = 1\n");
    let after = format!("changed = 2\n{block}{functions}{block}changed = 2\n");
    let sides = shaped("a.py", &before, &after, 1);
    for source in [lhs(&sides), rhs(&sides)] {
        let collapsed: Vec<_> = source
            .regions
            .iter()
            .filter(|region| region.visibility.collapsed)
            .map(|region| {
                (
                    (region.range.start.line, region.range.end.line),
                    region.visibility.label.clone(),
                    is_fold(region),
                )
            })
            .collect();
        assert_eq!(
            collapsed,
            [((2, 24), "22 unchanged lines".to_owned(), true)]
        );
        assert_eq!(
            open_lines(&source.regions),
            BTreeSet::from([0, 1, 2, 24, 25])
        );
    }
}

/// Lines of leaves that no collapsed region hides. Unlike `open_lines`,
/// a collapsed fold's header does not count: it shows the fold, not
/// context.
fn open_leaf_lines(regions: &[tree::Region]) -> BTreeSet<u32> {
    fn visit(regions: &[tree::Region], out: &mut BTreeSet<u32>) {
        for region in regions.iter().filter(|region| !region.visibility.collapsed) {
            match &region.node {
                tree::Node::Leaf { .. } => {
                    out.extend(region.range.start.line..region.range.end.line)
                }
                tree::Node::Fold { children } => visit(children, out),
            }
        }
    }
    let mut out = BTreeSet::new();
    visit(regions, &mut out);
    out
}

/// The text of every line of `source` that `open` holds.
fn open_text<'a>(source: &'a str, open: &BTreeSet<u32>) -> Vec<&'a str> {
    source
        .lines()
        .enumerate()
        .filter(|(line, _)| open.contains(&(*line as u32)))
        .map(|(_, text)| text)
        .collect()
}

#[test]
fn every_enclosing_scope_keeps_its_first_and_last_line() {
    let before = "class C:\n    def changed(self):\n        if False:\n            return\n        x = 1\n        x += 1\n        x += 1\n        x += 1\n        x += 1\n\n    def unrelated(self):\n        return 999\n";
    let after = before.replace("x = 1", "x = 2");
    let sides = shaped("a.py", before, &after, 0);
    let open = open_leaf_lines(&rhs(&sides).regions);
    // The class body holds the change, so its first and last line stay: the
    // last is the final line of the method below, not a closing brace
    // Python does not have.
    assert!(open.contains(&0) && open.contains(&11), "{open:?}");
    // The method that holds no change keeps nothing of its own; its body
    // collapses apart from the class's last line.
    assert!(!open.contains(&5) && !open.contains(&6), "{open:?}");
}

#[test]
fn generator_declarations_keep_their_signature_and_closing_brace() {
    let body = repeated(0..30, |i| format!("    yield value_{i};\n"));
    for extension in ["js", "jsx", "ts", "tsx"] {
        for prefix in ["export function*", "export async function*"] {
            let before = format!("{prefix} stream(\n    chunks,\n) {{\n{body}}}\n");
            let after = before.replace("yield value_20;", "yield changed_value;");
            let sides = shaped(&format!("stream.{extension}"), &before, &after, 0);
            for source in [lhs(&sides), rhs(&sides)] {
                let open = open_leaf_lines(&source.regions);
                assert!(
                    (0..3).all(|line| open.contains(&line)),
                    "missing generator signature: {extension}, {prefix}: {open:?}"
                );
                assert!(
                    open.contains(&33),
                    "missing closing brace: {extension}, {prefix}"
                );
                assert!(!open.contains(&10), "unrelated body stays collapsed");
            }
        }
    }
}

#[test]
fn a_multiline_python_signature_keeps_the_line_it_starts_on() {
    // A scope is the whole `def`, so the line it keeps is the one the
    // signature starts on, however far the `:` that opens the body is below
    // it. The rest of the signature is ordinary unchanged lines.
    let body = repeated(0..30, |i| format!("    value_{i} = {i}\n"));
    let before = format!("def run(\n    first,\n    second,\n):\n{body}");
    let after = before.replace("value_20 = 20", "value_20 = 999");
    let sides = shaped("a.py", &before, &after, 0);
    for source in [lhs(&sides), rhs(&sides)] {
        let open = open_leaf_lines(&source.regions);
        assert!(open.contains(&0), "{open:?}");
        assert!((1..4).all(|line| !open.contains(&line)), "{open:?}");
    }
}

#[test]
fn a_signature_keeps_its_header_without_neighbouring_statements() {
    let before = include_str!("../../../examples/review/real/02-review-175/before.ts");
    let after = include_str!("../../../examples/review/real/02-review-175/after.ts");
    let sides = shaped("a.ts", before, after, 3);
    let open = open_text(after, &open_leaf_lines(&rhs(&sides).regions));
    let shows = |text: &str| open.iter().any(|line| line.contains(text));
    assert!(shows("function parseReviewDiffFile("));
    assert!(!shows("return sections.filter("));
    assert!(!shows("const lines = section.split("));
    assert!(shows("let additions = 0;"), "changes keep ordinary context");
}

#[test]
fn an_unrelated_tail_return_is_not_context() {
    let before = include_str!("../../../examples/review/real/07-ripgrep-3487/before.rs");
    let after = include_str!("../../../examples/review/real/07-ripgrep-3487/after.rs");
    let shows = |lines: u32, text: &str| {
        let sides = shaped("a.rs", before, after, lines);
        let open = open_leaf_lines(&rhs(&sides).regions);
        open_text(after, &open)
            .iter()
            .any(|line| line.contains(text))
    };
    assert!(
        !shows(0, "Ok(if matched"),
        "the unrelated return is not context"
    );
    assert!(shows(3, "fn run("));
}

#[test]
fn a_changed_entry_keeps_its_enclosing_return_open_to_the_closer() {
    let entries = repeated(0..24, |i| format!("        '{i}': {i},\n"));
    let before = format!("def values():\n    return {{\n{entries}    }}\n");
    let after = before.replace("'12': 12", "'12': 999");
    let sides = shaped("a.py", &before, &after, 0);
    let open = open_leaf_lines(&rhs(&sides).regions);
    assert!(open.contains(&1), "the return opener is beyond the padding");
    assert!(
        open.contains(&26),
        "the dictionary's closer is beyond the padding"
    );
}

#[test]
fn a_scope_keeps_the_line_it_closes_on() {
    // A scope is the whole construct, so the line its `}` sits on is the
    // scope's own last line, whatever column the brace is in. The body fold
    // inside it ends above that line.
    let body = repeated(0..20, |i| format!("    let x{i} = {i};\n"));
    let tail = repeated(0..10, |i| format!("    let y{i} = {i};\n"));
    let before = format!("fn changed() {{\n{body}}}\n\nfn unrelated() {{\n{tail}}}\n");
    let after = before.replace("let x1 = 1;", "let x1 = 999;");
    let sides = shaped("a.rs", &before, &after, 0);
    for source in [lhs(&sides), rhs(&sides)] {
        let open = open_leaf_lines(&source.regions);
        assert!(open.contains(&0), "the header stays: {open:?}");
        assert!(open.contains(&2), "the change stays: {open:?}");
        assert!(
            !open.contains(&20),
            "the body's last line is not the scope's: {open:?}"
        );
        assert!(
            open.contains(&21),
            "the closing `}}` the scope ends on stays: {open:?}"
        );
        assert!(!open.contains(&10), "the unchanged body collapses");
        assert!(
            !open.contains(&23),
            "the untouched function below collapses: {open:?}"
        );
    }
}
