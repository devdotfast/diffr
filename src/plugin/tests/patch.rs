//! `--format patch` on files shaped by the bundled plugins.
use super::*;
use crate::protocol::stream::{write_file, Format, Options};
use crate::summary::DiffResult;

/// The patch of one file the way `--no-index --format patch` writes it.
fn patch_with(
    path: &str,
    before: &str,
    after: &str,
    params: &Params,
    pipeline: &Pipeline,
    options: DiffOptions,
    expand: bool,
) -> String {
    let mut output = Vec::new();
    let ended = write_file(
        path,
        path,
        (before.len() as u64, after.len() as u64),
        || DiffResult::from_sources_with_options(path, before, after, params, &options),
        params,
        pipeline,
        Options {
            syntax: false,
            updates: false,
            annotations: true,
            format: Format::Patch { expand },
        },
        &mut output,
    )
    .unwrap();
    assert!(!ended.failed && !ended.aborted);
    String::from_utf8(output).unwrap()
}

/// With the default configuration and plugins.
fn patch(path: &str, before: &str, after: &str) -> String {
    configured_patch("", path, before, after, DiffOptions::default(), false)
}

/// With the default plugins configured by `toml`.
fn configured_patch(
    toml: &str,
    path: &str,
    before: &str,
    after: &str,
    options: DiffOptions,
    expand: bool,
) -> String {
    let config = Config::from_toml(toml).unwrap();
    let pipeline = Pipeline::from_config(&config.plugins, Path::new(".")).unwrap();
    let params = config.compile_with(&pipeline).unwrap();
    patch_with(path, before, after, &params, &pipeline, options, expand)
}

/// `n` numbered statements, one per line, indented for a body.
fn statements(prefix: &str, n: u32) -> String {
    (1..=n)
        .map(|i| format!("        let {prefix}{i} = {i};\n"))
        .collect()
}

/// An impl with an unchanged method above a changed one.
fn parser(call: &str) -> String {
    format!(
        "struct Parser;\n\nimpl Parser {{\n    fn keep(&self) {{\n{}    }}\n\n    fn parse(&self) {{\n        self.{call}();\n        self.same();\n    }}\n}}\n",
        statements("a", 12)
    )
}

/// A changed call between five unchanged statements on either side.
fn padded(call: &str) -> String {
    format!(
        "fn run() {{\n{}        {call}();\n{}}}\n",
        statements("a", 5),
        statements("b", 5)
    )
}

#[test]
fn changed_lines_print_between_numbered_context_and_folds_as_one_marker() {
    assert_eq!(
        patch("src/lib.rs", &parser("old"), &parser("new")),
        "diff --git a/src/lib.rs b/src/lib.rs
 1  1  struct Parser;
 2  2  
 3  3  impl Parser {
@@ … 13 unchanged lines · impl Parser › fn keep(&self) @@
17 17      }
18 18  
19 19      fn parse(&self) {
20    -        self.old();
   20 +        self.new();
21 21          self.same();
22 22      }
23 23  }
"
    );
}

#[test]
fn consecutive_unchanged_regions_add_up_under_the_scopes_they_share() {
    let method = |name: &str| format!("    fn {name}(&self) {{\n{}    }}\n\n", statements("a", 8));
    let file = |call: &str| {
        format!(
            "impl Parser {{\n{}{}{}    fn parse(&self) {{\n        self.{call}();\n    }}\n}}\n",
            method("a"),
            method("b"),
            method("c")
        )
    };
    let markers = |toml: &str| {
        let text = configured_patch(
            toml,
            "src/lib.rs",
            &file("old"),
            &file("new"),
            DiffOptions::default(),
            false,
        );
        text.lines()
            .filter(|line| line.starts_with("@@"))
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        markers("[plugins.bundled.group]\nenabled = false\n"),
        ["@@ … 31 unchanged lines · impl Parser @@"]
    );
    // The group plugin makes the run one region of its own.
    assert_eq!(
        markers(""),
        ["@@ … 2 collapsed regions · 32 lines · impl Parser @@"]
    );
}

#[test]
fn unified_sets_the_context_lines_kept_around_a_change() {
    let unified = |lines: u32| {
        configured_patch(
            // The group plugin would join the two gaps around the change.
            &format!("[plugins.bundled.context]\nlines = {lines}\n[plugins.bundled.group]\nenabled = false\n"),
            "src/lib.rs",
            &padded("old"),
            &padded("new"),
            DiffOptions::default(),
            false,
        )
    };
    // The enclosing scope's first and last lines stay whatever the count.
    assert_eq!(
        unified(0),
        "diff --git a/src/lib.rs b/src/lib.rs
 1  1  fn run() {
@@ … 5 unchanged lines · fn run() @@
 7    -        old();
    7 +        new();
@@ … 5 unchanged lines · fn run() @@
13 13  }
"
    );
    assert_eq!(
        unified(1),
        "diff --git a/src/lib.rs b/src/lib.rs
 1  1  fn run() {
@@ … 4 unchanged lines · fn run() @@
 6  6          let a5 = 5;
 7    -        old();
    7 +        new();
 8  8          let b1 = 1;
@@ … 4 unchanged lines · fn run() @@
13 13  }
"
    );
}

#[test]
fn a_deleted_body_is_one_marker_until_folds_are_expanded() {
    let before = format!(
        "fn keep() {{}}\n\nfn gone() {{\n{}}}\n",
        statements("g", 12)
    );
    let after = "fn keep() {}\n";
    assert_eq!(
        patch("src/lib.rs", &before, after),
        "diff --git a/src/lib.rs b/src/lib.rs
 1  1  fn keep() {}
 2    -
 3    -fn gone() {
@@ … 12 lines removed · fn gone() @@
16    -}
"
    );
    let expanded = configured_patch(
        "",
        "src/lib.rs",
        &before,
        after,
        DiffOptions::default(),
        true,
    );
    assert!(!expanded.contains("@@"), "{expanded}");
    assert_eq!(expanded.matches("    -").count(), 15, "{expanded}");
    assert!(
        expanded.contains("15    -        let g12 = 12;\n"),
        "{expanded}"
    );
}

#[test]
fn a_file_that_fell_back_to_a_line_diff_says_why() {
    let text = configured_patch(
        "",
        "src/lib.rs",
        &parser("old"),
        &parser("new"),
        DiffOptions {
            graph_limit: 1,
            ..DiffOptions::default()
        },
        false,
    );
    let mut lines = text.lines().skip(1);
    assert_eq!(
        lines.next(),
        Some("Line diff (too_complex): structural diff exceeded diff.graph_limit (1); raise it in diffr config")
    );
    assert!(
        text.contains("20    -        self.old();\n   20 +        self.new();\n"),
        "{text}"
    );
    // A file without a grammar is always a line diff, which goes unsaid.
    let text = patch("notes.txt", "a\n", "b\n");
    assert_eq!(text, "diff --git a/notes.txt b/notes.txt\n1   -a\n  1 +b\n");
}

#[test]
fn a_summary_prints_under_its_marker() {
    let after = "/// Sums.\n/// Twice.\nfn total(a: u32, b: u32, c: u32) -> u32 {\n    let x = a;\n    let y = b;\n    let z = c;\n    x + y + z\n}\n";
    let params =
        Config::from_toml("[plugins.bundled.summarize]\nenabled = true\napi_key = 'test'\n")
            .unwrap()
            .compile()
            .unwrap();
    let (_, sides) = project_compiled("a.rs", "", after, &params, DiffOptions::default());
    let id = diffr_plugin_summarize::select(&trees(&sides), 3, None)[0].0;
    let (endpoint, server) = super::summarize::serve(vec![(
        200,
        super::summarize::gemini_answer(&[(id, "x, y, z = a, b, c\nreturn x + y + z")]),
    )]);
    let pipeline = super::summarize::summarizer(&endpoint, 0);
    let text = patch_with(
        "a.rs",
        "",
        after,
        &params,
        &pipeline,
        DiffOptions::default(),
        false,
    );
    server.join().unwrap();
    assert_eq!(
        text,
        "diff --git a/a.rs b/a.rs
@@ … 2 lines of documentation · fn total(a: u32, b: u32, c: u32) -> u32 @@
  3 +fn total(a: u32, b: u32, c: u32) -> u32 {
@@ … 4 lines, summarized · fn total(a: u32, b: u32, c: u32) -> u32 @@
    ~    x, y, z = a, b, c
    ~    return x + y + z
  8 +}
"
    );
}

/// Hand-built regions, for fold shapes the bundled plugins make only in
/// larger files.
mod nested {
    use super::*;
    use crate::protocol::{
        Diff, LineCounts, Node, Outcome, Region, SourcePos, SourceRange, Stats, StructuralChanges,
        Visibility,
    };

    fn region(id: u32, lines: (u32, u32), collapsed: bool, label: &str, node: Node) -> Region {
        Region {
            id,
            fold_state_id: id,
            range: SourceRange {
                start: SourcePos {
                    line: lines.0,
                    column: 0,
                },
                end: SourcePos {
                    line: lines.1,
                    column: 0,
                },
            },
            tags: vec![],
            visibility: Visibility {
                collapsed,
                label: label.to_owned(),
            },
            node,
        }
    }

    fn leaf(id: u32, lines: (u32, u32), collapsed: bool, changed: &[u32]) -> Region {
        let node = Node::Leaf {
            alignment_id: id,
            changed: changed
                .iter()
                .map(|&line| crate::protocol::Span {
                    line,
                    start_column: 0,
                    end_column: 1,
                })
                .collect(),
        };
        region(id, lines, collapsed, "", node)
    }

    fn fold(
        id: u32,
        lines: (u32, u32),
        collapsed: bool,
        label: &str,
        children: Vec<Region>,
    ) -> Region {
        region(id, lines, collapsed, label, Node::Fold { children })
    }

    /// Ten added lines `l0`..`l9` with these regions.
    fn render(regions: Vec<Region>) -> String {
        let text: String = (0..10).map(|line| format!("l{line}\n")).collect();
        let entry = FileChange {
            file: Pairing::RightOnly {
                rhs: FileRef {
                    path: "a.txt".to_owned(),
                    oid: String::new(),
                    mode: "100644".to_owned(),
                },
            },
            status: FileStatus::Added,
            tags: Vec::new(),
        };
        let counts = LineCounts {
            added: 10,
            removed: 0,
        };
        let outcome = Outcome::Diff {
            diff: Diff::Text {
                sides: Pairing::RightOnly {
                    rhs: protocol::Source {
                        text,
                        syntax: Vec::new(),
                        regions,
                    },
                },
                stats: Stats {
                    textual: counts,
                    visible: counts,
                    fallback: None,
                },
                structural_changes: StructuralChanges {
                    base: vec![],
                    head: vec![[0, 10]],
                },
            },
        };
        let mut out = String::new();
        crate::protocol::patch::render(&entry, &Visibility::default(), &outcome, false, &mut out);
        out
    }

    #[test]
    fn a_collapsed_group_hides_the_folds_inside_it() {
        let text = render(vec![
            leaf(1, (0, 2), false, &[]),
            fold(
                2,
                (2, 8),
                true,
                "2 collapsed regions · 6 lines",
                vec![
                    fold(
                        3,
                        (2, 5),
                        true,
                        "3 lines removed",
                        vec![leaf(4, (2, 5), false, &[])],
                    ),
                    leaf(5, (5, 6), false, &[]),
                    fold(6, (6, 8), true, "", vec![leaf(7, (6, 8), false, &[])]),
                ],
            ),
            leaf(8, (8, 10), false, &[]),
        ]);
        assert_eq!(
            text,
            "diff --git a/a.txt b/a.txt
new file mode 100644
    1 +l0
    2 +l1
@@ … 2 collapsed regions · 6 lines @@
    9 +l8
   10 +l9
"
        );
    }

    #[test]
    fn an_open_fold_shows_its_lines_and_the_collapsed_folds_inside() {
        let text = render(vec![fold(
            1,
            (0, 10),
            false,
            "",
            vec![
                leaf(2, (0, 3), false, &[]),
                fold(3, (3, 6), true, "", vec![leaf(4, (3, 6), false, &[])]),
                leaf(5, (6, 7), true, &[]),
                leaf(6, (7, 10), false, &[]),
            ],
        )]);
        assert_eq!(
            text,
            "diff --git a/a.txt b/a.txt
new file mode 100644
    1 +l0
    2 +l1
    3 +l2
@@ … 3 lines @@
@@ … 1 line @@
    8 +l7
    9 +l8
   10 +l9
"
        );
    }
}
