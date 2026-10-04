use super::*;
use crate::protocol::{SourcePos, SourceRange, Span, Visibility};
use std::collections::BTreeSet;

/// One rendered line per value, concatenated.
fn repeated(values: impl Iterator<Item = u32>, render: impl Fn(u32) -> String) -> String {
    let mut out = String::new();
    for value in values {
        out.push_str(&render(value));
    }
    out
}

fn shaped(path: &str, before: &str, after: &str, lines: u32) -> Pairing<Source> {
    let (file, mut sides) = project(path, before, after);
    run("context", json!({ "lines": lines }), &file, &mut sides);
    sides.clone()
}

/// Every region's lines and whether it starts collapsed, with its label,
/// in document order.
fn rows(regions: &[Region]) -> Vec<((u32, u32), bool, String, bool)> {
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
fn open_lines(regions: &[Region]) -> BTreeSet<u32> {
    fn visit(regions: &[Region], out: &mut BTreeSet<u32>) {
        for region in regions {
            if region.visibility.collapsed {
                continue;
            }
            match &region.node {
                Node::Leaf { .. } => out.extend(region.range.start.line..region.range.end.line),
                Node::Fold { children } => visit(children, out),
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
    assert_eq!(open_lines(&lhs(&sides).regions), (17..24).collect());
    assert_eq!(outer_gaps(lhs(&sides)), [0..17, 24..41]);
    let sides = shaped("a.py", &before, &after, 1);
    assert_eq!(open_lines(&rhs(&sides).regions), (19..22).collect());
    assert_eq!(outer_gaps(rhs(&sides)), [0..19, 22..41]);
}

#[test]
fn short_gaps_inside_a_scope_fold_too() {
    // Fold length is not a readability heuristic: even this two-line gap folds.
    let head = repeated(0..8, |i| format!("x{i} = {i}\n"));
    let before = format!("{head}\ndef f():\n    a = 1\n    b = 1\n    c = 1\n    return 1\n");
    let after = format!("{head}\ndef f():\n    a = 1\n    b = 1\n    c = 1\n    return 2\n");
    let sides = shaped("a.py", &before, &after, 1);
    assert_eq!(outer_gaps(rhs(&sides)), [0..9, 10..12]);
}

#[test]
fn the_enclosing_header_stays_open_above_a_deep_change() {
    // The whole multiline signature remains readable above a distant change.
    let head = repeated(0..10, |i| format!("const C{i}: u32 = {i};\n"));
    let body = repeated(0..10, |i| format!("    let a{i} = {i};\n"));
    let signature = "fn outer(\n    a: u32,\n    b: u32,\n    c: u32,\n) -> u32 {\n";
    let before = format!("{head}{signature}{body}    1\n}}\n");
    let after = format!("{head}{signature}{body}    2\n}}\n");
    let sides = shaped("a.rs", &before, &after, 1);
    let open = open_lines(&rhs(&sides).regions);
    assert_eq!(open, BTreeSet::from([10, 11, 12, 13, 14, 24, 25, 26]));
    // Without a context query the whole signature collapses with the
    // unchanged lines above it.
    let config = Config::default();
    let queries = config
        .plugins
        .queries()
        .unwrap()
        .into_iter()
        .filter(|(plugin, _)| plugin != "context")
        .collect();
    let params = config.compile_queries(queries).unwrap();
    let (file, mut sides) =
        project_compiled("a.rs", &before, &after, &params, DiffOptions::default());
    run("context", json!({"lines": 1}), &file, &mut sides);
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
            by_line: Some(crate::summary::FallbackCause::Generated),
            ..DiffOptions::default()
        },
    );
    let mut scopes = 0;
    walk(&rhs(&sides).regions, &mut |region| {
        scopes += usize::from(has_tag(region, "context:scope"));
    });
    assert_eq!(scopes, 0);
    run("context", json!({"lines": 1}), &file, &mut sides);
    assert_eq!(
        open_lines(&rhs(&sides).regions),
        BTreeSet::from([10, 11, 12])
    );
}

#[test]
fn a_stretch_crossing_a_fold_end_is_one_root_interval() {
    // The gap crosses the inner block's boundary and still has one outer fold.
    let body = repeated(1..=7, |n| format!("        u{n}();\n"));
    let tail = repeated(1..=5, |n| format!("    v{n}();\n"));
    let before = format!("fn f() {{\n    if a {{\n        x();\n{body}    }}\n{tail}}}\n");
    let after = format!("fn f() {{\n    if a {{\n        y();\n{body}    }}\n{tail}}}\n");
    let sides = shaped("a.rs", &before, &after, 1);
    for source in [lhs(&sides), rhs(&sides)] {
        assert_eq!(outer_gaps(source), [4..16]);
        let open = open_lines(&source.regions);
        assert!(open.contains(&0) && open.contains(&16), "{open:?}");
    }
}

#[test]
fn short_unchanged_files_fold_and_one_sided_changes_stay_open() {
    let sides = shaped("a.py", "x = 1\ny = 2\n", "x = 1\ny = 2\n", 3);
    assert_eq!(outer_gaps(lhs(&sides)), [0..2]);
    assert!(open_lines(&lhs(&sides).regions).is_empty());
    assert_eq!(
        lhs(&sides).regions[0].fold_state_id,
        rhs(&sides).regions[0].fold_state_id
    );
    let (file, sides) = project("a.py", "", "def f():\n    return 1\n");
    let Pairing::Both { rhs: after, .. } = sides.clone() else {
        panic!("both sides");
    };
    let mut sides = Pairing::RightOnly { rhs: after };
    run("context", json!({"lines": 3}), &file, &mut sides);
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
    let open = open_lines(&rhs(&sides).regions);
    // The parse's folds stand, so the changed function's header does.
    assert!(open.contains(&0), "{open:?}");
    assert!(!open.contains(&2), "{open:?}");
    // Unmatched folds are handled independently on each side.
    let Pairing::Both { lhs: before, .. } = &sides else {
        panic!("both sides")
    };
    assert!(!open_lines(&before.regions).contains(&10));
    assert!(!open.contains(&10));
}

#[test]
fn a_fold_whose_matched_partner_holds_changes_stays_open() {
    // Fold 1 on the lhs lies inside an unchanged stretch, but its
    // matched partner on the rhs (fold 5, sharing the fold state) moved
    // below and holds new lines. Collapsing fold 1 would hide it.
    let range = |start: u32, end: u32| SourceRange {
        start: SourcePos {
            line: start,
            column: 0,
        },
        end: SourcePos {
            line: end,
            column: 0,
        },
    };
    let leaf = |id: u32, alignment: u32, start: u32, end: u32, changed: bool| Region {
        id,
        fold_state_id: id,
        range: range(start, end),
        relations: Vec::new(),
        tags: vec![],
        visibility: Visibility::default(),
        node: Node::Leaf {
            alignment_id: alignment,
            changed: (start..end)
                .filter(|_| changed)
                .map(|line| Span {
                    line,
                    start_column: 0,
                    end_column: 1,
                })
                .collect(),
        },
    };
    // The rhs leaf paired with the lhs leaf `lhs`, whose id is also its
    // alignment id.
    let paired = |id: u32, lhs: u32, start: u32, end: u32, changed: bool| Region {
        fold_state_id: lhs,
        ..leaf(id, lhs, start, end, changed)
    };
    let fold = |id: u32, state: u32, child: Region| Region {
        id,
        fold_state_id: state,
        range: child.range,
        relations: Vec::new(),
        tags: vec![],
        visibility: Visibility::default(),
        node: Node::Fold {
            children: vec![child],
        },
    };
    let source = |lines: usize, regions| Source {
        syntax: Vec::new(),
        text: "x\n".repeat(lines),
        regions,
    };
    let mut sides = Pairing::Both {
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
    run("context", json!({"lines": 1}), &file, &mut sides);
    assert!((6..10).all(|line| open_lines(&rhs(&sides).regions).contains(&line)));
}

/// context.
fn open_leaf_lines(regions: &[Region]) -> BTreeSet<u32> {
    fn visit(regions: &[Region], out: &mut BTreeSet<u32>) {
        for region in regions.iter().filter(|region| !region.visibility.collapsed) {
            match &region.node {
                Node::Leaf { .. } => out.extend(region.range.start.line..region.range.end.line),
                Node::Fold { children } => visit(children, out),
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

#[test]
fn exported_tsx_functions_keep_complete_headers_above_distant_changes() {
    let body = repeated(0..30, |i| format!("    const value_{i} = {i};\n"));
    let before = format!("export function ReviewHome({{\n    reviews,\n    setup,\n}}: Props) {{\n{body}    return <h1>Reviews</h1>;\n}}\n");
    let after = before.replace("<h1>Reviews</h1>", "<h1>Sessions</h1>");
    let sides = shaped("home.tsx", &before, &after, 3);
    for source in [lhs(&sides), rhs(&sides)] {
        let open = open_leaf_lines(&source.regions);
        assert!(
            (0..4).all(|line| open.contains(&line)),
            "header hidden: {open:?}"
        );
        assert!(!open.contains(&10), "unrelated body should collapse");
        assert!(open.contains(&34), "changed JSX should stay visible");
    }
}

/// Context puts every initially hidden interval at the root.
fn outer_gaps(source: &Source) -> Vec<std::ops::Range<u32>> {
    source
        .regions
        .iter()
        .filter(|r| r.visibility.collapsed)
        .map(|r| r.range.lines())
        .collect()
}

#[test]
fn opposite_insertions_split_zero_context_gaps() {
    for (before, after) in [
        ("a\nb\nc\nd\n", "a\nb\ninserted\nc\nd\n"),
        ("a\nb\ninserted\nc\nd\n", "a\nb\nc\nd\n"),
    ] {
        let sides = shaped("a.txt", before, after, 0);
        for source in [lhs(&sides), rhs(&sides)] {
            let short = source.text.lines().count() == 4;
            assert_eq!(
                outer_gaps(source),
                if short {
                    vec![0..2, 2..4]
                } else {
                    vec![0..2, 3..5]
                }
            );
            assert_eq!(
                open_lines(&source.regions),
                if short {
                    BTreeSet::new()
                } else {
                    BTreeSet::from([2])
                }
            );
        }
    }
}

#[test]
fn an_interval_starting_inside_an_arrow_keeps_nested_callback_bodies() {
    let before = "export function render() {\n  const changed = 1;\n  const view = useState(() =>\n    normalizeView(\n      \"review\",\n      enabled,\n      active,\n    ),\n  );\n\n  useEffect(() => {\n    refresh();\n  }, []);\n\n}\n";
    let after = before.replace("changed = 1", "changed = 2");
    let sides = shaped("view.tsx", before, &after, 2);
    for source in [lhs(&sides), rhs(&sides)] {
        assert_eq!(outer_gaps(source), [4..14]);
        let root = source
            .regions
            .iter()
            .find(|r| r.visibility.collapsed)
            .unwrap();
        let Node::Fold { children } = &root.node else {
            panic!("outer fold")
        };
        let visible = open_lines(children);
        assert!(
            visible.contains(&4),
            "opening the gap reveals the arguments"
        );
        assert!(visible.contains(&10), "the callback header stays visible");
        assert!(!visible.contains(&11), "the callback body stays folded");
    }
}

#[test]
fn expanding_an_outer_gap_reveals_headers_with_collapsed_ast_bodies() {
    let before = "def changed():\n    return 1\n\ndef spare():\n    for item in items:\n        visit(item)\n    finish()\n";
    let after = before.replace("return 1", "return 2");
    let mut sides = shaped("a.py", before, &after, 0);
    let Pairing::Both { rhs, .. } = &mut sides else {
        panic!("both sides")
    };
    let root = rhs
        .regions
        .iter_mut()
        .find(|r| r.range.lines().contains(&3))
        .unwrap();
    assert!(root.visibility.collapsed);
    root.visibility.collapsed = false;
    let visible = open_lines(&rhs.regions);
    assert!(
        visible.contains(&3),
        "function header must be readable on first expansion: {visible:?}"
    );
    assert!(!visible.contains(&5), "nested body remains folded");
    walk_mut(&mut rhs.regions, &mut |r| {
        if r.range.lines() == (4..7) {
            r.visibility.collapsed = false;
        }
    });
    let visible = open_lines(&rhs.regions);
    assert!(
        visible.contains(&4),
        "opening the function reveals its for-loop header"
    );
    assert!(
        !visible.contains(&5),
        "the for-loop body remains collapsed for the next expansion"
    );
}

#[test]
fn descriptive_summaries_remain_visible_between_outer_gaps() {
    let before = "def changed():\n    return 1\n\ndef spare():\n    first()\n    second()\n    third()\n\ndef last():\n    finish()\n";
    let after = before.replace("return 1", "return 2");
    let (file, mut sides) = project("a.py", before, &after);
    let Pairing::Both { lhs, rhs } = &mut sides else {
        panic!("both sides")
    };
    for source in [lhs, rhs] {
        walk_mut(&mut source.regions, &mut |r| {
            if has_tag(r, "deleted-bodies:function") && r.range.start.line == 4 {
                r.visibility.collapsed = true;
                r.visibility.label = "Prepare and save results".into();
            }
        });
    }
    run("context", json!({"lines": 0}), &file, &mut sides);
    fn labels(regions: &[Region], out: &mut Vec<String>) {
        for r in regions {
            if r.visibility.collapsed {
                out.push(r.visibility.label.clone());
            } else if let Node::Fold { children } = &r.node {
                labels(children, out);
            }
        }
    }
    for source in [super::lhs(&sides), super::rhs(&sides)] {
        let mut visible = Vec::new();
        labels(&source.regions, &mut visible);
        assert!(
            visible.iter().any(|s| s == "Prepare and save results"),
            "{visible:?}"
        );
        assert!(
            visible
                .iter()
                .filter(|s| s.ends_with("unchanged lines"))
                .count()
                >= 2
        );
    }
}

#[test]
fn expanding_context_preserves_single_and_multiline_tsx_callback_folds() {
    let before = "const changed = 1;\nuseEffect(() => {\n  first();\n}, []);\nuseEffect(() => {\n  second();\n  third();\n}, []);\n";
    let after = before.replace("changed = 1", "changed = 2");
    let mut sides = shaped("effects.tsx", before, &after, 0);
    let Pairing::Both { lhs, rhs } = &mut sides else {
        panic!("both sides")
    };
    for source in [lhs, rhs] {
        for root in &mut source.regions {
            root.visibility.collapsed = false;
        }
        let visible = open_lines(&source.regions);
        assert!(
            visible.contains(&1) && visible.contains(&4),
            "callback headers: {visible:?}"
        );
        assert!(
            !visible.contains(&2) && !visible.contains(&5),
            "callback bodies stay folded: {visible:?}"
        );
        walk_mut(&mut source.regions, &mut |r| {
            if r.range.lines() == (2..3) {
                r.visibility.collapsed = false;
            }
        });
        let visible = open_lines(&source.regions);
        assert!(
            visible.contains(&2),
            "the one-line callback expands independently"
        );
        assert!(
            !visible.contains(&5),
            "the neighboring callback remains collapsed"
        );
    }
}

#[test]
fn changed_inline_callbacks_keep_the_statement_header_and_closer_with_zero_context() {
    for (header, closer) in [
        ("useEffect(() => {", "});"),
        ("const result = useMemo(() => {", "}, []);"),
        ("const callback = () => {", "};"),
    ] {
        let before = format!(
            "{header}\n  const x = 1;\n  const y = 1;\n  const z = 1;\n  work(1);\n  other();\n{closer}\n"
        );
        let after = before.replace("work(1)", "work(2)");
        let sides = shaped("callback.tsx", &before, &after, 0);
        for source in [lhs(&sides), rhs(&sides)] {
            assert_eq!(
                open_lines(&source.regions),
                BTreeSet::from([0, 4, 6]),
                "callback header, change and closer: {header}"
            );
            assert_eq!(outer_gaps(source), [1..4, 5..6]);
        }
    }
}
