//! End-to-end annotation checks through the experimental source adapter.
use crate::lines::{SourcePosition, SourceRange};

fn text<'a>(source: &'a str, range: &SourceRange) -> &'a str {
    let offset = |p: &SourcePosition| {
        let line = p.line.as_usize();
        let prefix = source
            .split_inclusive('\n')
            .take(line)
            .map(str::len)
            .sum::<usize>();
        prefix + p.byte_column
    };
    let start = offset(&range.start);
    let end = offset(&range.end);
    assert!(start < end, "nonempty ranges");
    &source[start..end] // Bounds and UTF-8 boundary validation.
}

mod folds {
    use super::text;
    use crate::lines::SourceRange;
    use crate::parse::folds::{Correspondence, Fold, FoldKind};
    use crate::summary::DiffResult;
    use std::fmt::Write as _;

    fn paired(fold: &Fold) -> Option<(&SourceRange, &SourceRange)> {
        match &fold.regions {
            Correspondence::Paired { lhs, rhs } => Some((lhs, rhs)),
            _ => None,
        }
    }
    fn added(fold: &Fold) -> Option<&SourceRange> {
        match &fold.regions {
            Correspondence::Added(range) => Some(range),
            _ => None,
        }
    }

    #[test]
    fn renamed_function_with_changed_body_uses_structural_correspondence() {
        let lhs = "fn run() {\n    old_work();\n}\n";
        let rhs = "fn execute() {\n    old_work();\n    new_work();\n}\n";
        let diff = DiffResult::from_sources("a.rs", lhs, rhs);
        assert!(diff.folds.iter().any(|f| {
            paired(f).is_some_and(|(l, r)| {
                l.start.line.as_usize() == 0
                    && l.end.line.as_usize() == 2
                    && r.end.line.as_usize() == 3
            })
        }));
    }

    #[test]
    fn call_arguments_are_not_fold_candidates() {
        let diff =
            DiffResult::from_sources("a.rs", "", "fn run() {\n    call(123, other(456));\n}\n");
        assert_eq!(diff.folds.len(), 1);
    }

    #[test]
    fn multiline_atoms_and_test_bodies_keep_fold_metadata() {
        let src = "def test_read():\n    value = \"\"\"first\nsecond\"\"\"\n    return value\n";
        let result = DiffResult::from_sources("a.py", "", src);
        assert!(result.folds.iter().any(|fold| fold.placeholder == "Test"));
        assert!(result.folds.iter().any(|fold| fold.placeholder == "String"
            && added(fold).is_some_and(|r| text(src, r) == "\"\"\"first\nsecond\"\"\"")));
    }

    #[test]
    fn flattened_test_body_projects_once_using_the_existing_string_match() {
        let lhs = "def test_doc():\n    \"\"\"some shared words before\"\"\"\n";
        let rhs = "def test_doc():\n    \"\"\"some shared words after\"\"\"\n";
        let result = DiffResult::from_sources("a.py", lhs, rhs);
        assert_eq!(result.folds.len(), 1);
        let fold = &result.folds[0];
        assert_eq!(fold.placeholder, "Test");
        let (left, right) = paired(fold).expect("reuse the replaced-string correspondence");
        assert_eq!(text(lhs, left), "\"\"\"some shared words before\"\"\"");
        assert_eq!(text(rhs, right), "\"\"\"some shared words after\"\"\"");
    }

    #[test]
    fn imports_remain_individual_typed_folds() {
        let source = "import os\nimport sys\n";
        for (lhs, rhs) in [
            (
                "import os\nimport sys\nx = 1\n",
                "import os\nimport sys\nx = 2\n",
            ),
            ("", source),
            (source, ""),
        ] {
            let diff = DiffResult::from_sources("a.py", lhs, rhs);
            assert_eq!(diff.folds.len(), 2);
            for (fold, expected) in diff.folds.iter().zip(["import os", "import sys"]) {
                assert_eq!(fold.kind, FoldKind::Import);
                match &fold.regions {
                    Correspondence::Paired { lhs: l, rhs: r } => {
                        assert_eq!(text(lhs, l), expected);
                        assert_eq!(text(rhs, r), expected);
                    }
                    Correspondence::Added(r) => assert_eq!(text(rhs, r), expected),
                    Correspondence::Deleted(l) => assert_eq!(text(lhs, l), expected),
                }
            }
            assert_eq!(diff.domain_json()["folds"][0]["kind"], "Import");
        }
    }

    #[test]
    fn imports_outside_displayed_context_remain_available() {
        let mut body = String::new();
        for i in 0..30 {
            writeln!(body, "    value_{i} = {i}").unwrap();
        }
        let lhs = format!("import os\n\ndef run():\n{body}\n");
        let rhs = lhs.replace("value_29 = 29", "value_29 = 999");
        let diff = DiffResult::from_sources("a.py", &lhs, &rhs);
        assert!(diff
            .folds
            .iter()
            .any(|f| matches!(f.placeholder.as_str(), "Import" | "Imports")));
    }

    #[test]
    fn import_folds_remain_one_sided_when_file_added_or_deleted() {
        let source = "import os\n\ndef f():\n    return os.getcwd()\n";
        let added_diff = DiffResult::from_sources("a.py", "", source);
        assert!(matches!(
            &added_diff.folds[0].regions,
            Correspondence::Added(r) if text(source, r) == "import os"
        ));
        let deleted_diff = DiffResult::from_sources("a.py", source, "");
        assert!(matches!(
            &deleted_diff.folds[0].regions,
            Correspondence::Deleted(r) if text(source, r) == "import os"
        ));
    }

    #[test]
    fn nested_try_catch_bodies_pair_and_keep_delimiters() {
        let lhs = "function run() {\n  try {\n    work(1);\n  } catch (error) {\n    report(error);\n  }\n}\n";
        let rhs = lhs.replace("work(1)", "work(2)");
        let review = DiffResult::from_sources("a.ts", lhs, &rhs);
        for expected in [
            "\n  try {\n    work(1);\n  } catch (error) {\n    report(error);\n  }\n",
            "\n    work(1);\n  ",
            "\n    report(error);\n  ",
        ] {
            let expected_rhs = expected.replace("work(1)", "work(2)");
            assert!(
                review.folds.iter().any(|f| {
                    paired(f).is_some_and(|(l, r)| {
                        text(lhs, l) == expected && text(&rhs, r) == expected_rhs
                    })
                }),
                "missing paired fold for {expected:?}"
            );
        }
    }

    #[test]
    fn added_rust_test_body_is_foldable_without_hiding_signature_or_brace() {
        let lhs = include_str!("../../examples/review/real/08-ripgrep-3496/before.rs");
        let rhs = include_str!("../../examples/review/real/08-ripgrep-3496/after.rs");
        let review = DiffResult::from_sources("walk.rs", lhs, rhs);
        let signature = rhs
            .lines()
            .position(|l| l.contains("fn max_depth_does_not_load_unreachable_ignore_files()"))
            .unwrap();
        assert!(review.folds.iter().any(|f| {
            added(f).is_some_and(|r| {
                r.start.line.as_usize() == signature
                    && text(rhs, r).contains("let td = tmpdir();")
                    && rhs.lines().nth(r.end.line.as_usize()).unwrap().trim() == "}"
            })
        }));
    }

    #[test]
    fn python_suites_and_go_blocks_are_foldable() {
        for (path, src, expected) in [
            (
                "a.py",
                "def run():\n    try:\n        work()\n    except Exception:\n        recover()\n",
                "work()",
            ),
            (
                "a.go",
                "package main\nfunc run() {\n    if ready {\n        work()\n    }\n}\n",
                "\n        work()\n    ",
            ),
        ] {
            let review = DiffResult::from_sources(path, "", src);
            assert!(
                review
                    .folds
                    .iter()
                    .any(|f| added(f).is_some_and(|r| text(src, r) == expected)),
                "missing body fold in {path}"
            );
        }
    }

    #[test]
    fn inline_collections_have_exact_paired_byte_ranges() {
        let lhs = "const x = [\"☕\", oldValue];\n";
        let rhs = "const x = [\"☕\", newValue];\n";
        let review = DiffResult::from_sources("a.ts", lhs, rhs);
        assert!(review.folds.iter().any(|f| {
            paired(f).is_some_and(|(l, r)| {
                text(lhs, l) == "\"☕\", oldValue" && text(rhs, r) == "\"☕\", newValue"
            })
        }));
    }
}

mod syntax_tests {
    use super::text;
    use crate::display::hunks::ContextRange;
    use crate::display::line_layout::LineSelection;
    use crate::summary::DiffResult;
    use crate::summary::FileContent;
    use std::fmt::Write as _;
    #[test]
    fn distant_changes_share_one_hunk_and_one_context_copy() {
        let mut statements = String::new();
        for i in 0..70 {
            writeln!(statements, "    work({i});").unwrap();
        }
        let lhs = format!("fn run() {{\n{statements}}}\n");
        let rhs = lhs
            .replace("work(10)", "changed(10)")
            .replace("work(35)", "changed(35)")
            .replace("work(60)", "changed(60)");
        let diff = DiffResult::from_sources("a.rs", &lhs, &rhs);
        assert_eq!(diff.hunks.len(), 1);
        let mut seen = std::collections::BTreeSet::new();
        for context in &diff.hunks[0].context {
            for pair in context.lhs.rows().zip(context.rhs.rows()) {
                assert!(seen.insert(pair), "duplicate context row");
            }
        }
        assert!(seen.contains(&(0, 0)));
        assert!(seen.contains(&(71, 71)));
    }

    #[test]
    fn new_and_deleted_functions_have_folds_but_no_extra_context() {
        let source = "fn run() {\n    work();\n}\n";
        for (lhs, rhs) in [("", source), (source, "")] {
            let diff = DiffResult::from_sources("a.rs", lhs, rhs);
            assert!(!diff.folds.is_empty());
            assert!(diff.hunks.iter().all(|h| h.context.is_empty()));
        }
    }

    #[test]
    fn unicode_crlf_and_multiline_return_boundaries() {
        let lhs =
        "def café():\r\n    x = 1\r\n    return (\r\n        x,\r\n\r\n        '☕',\r\n    )\r\n";
        let rhs = lhs.replace("x = 1", "x = 2");
        let result = DiffResult::from_sources("a.py", lhs, &rhs);
        assert!(matches!(&result.rhs_src,FileContent::Text(s) if s==&rhs));
        for region in result.hunks.iter().flat_map(|h| &h.context) {
            text(lhs, &region.lhs);
            text(&rhs, &region.rhs);
        }
        assert!(result.snapshot().contains("café"));
    }

    #[test]
    fn python_class_has_no_synthetic_closing_context() {
        let lhs = "class C:\n    def changed(self):\n        if False:\n            return\n        x = 1\n        x += 1\n        x += 1\n        x += 1\n        x += 1\n\n    def unrelated(self):\n        return 999\n";
        let rhs = lhs.replace("x = 1", "x = 2");
        let result = DiffResult::from_sources("a.py", lhs, &rhs);
        assert!(!result.snapshot().contains("unrelated"));
        assert!(!result.snapshot().contains("return 999"));
    }

    #[test]
    fn context_belongs_to_the_hunk_that_requested_it() {
        let function = |name: &str| {
            format!(
                "fn {name}() {{\n{}    return;\n}}\n",
                "    work();\n".repeat(24)
            )
        };
        let lhs = format!("{}\n{}", function("first"), function("second"));
        let mut lines: Vec<_> = lhs.lines().map(str::to_owned).collect();
        let second = lines.iter().position(|l| l == "fn second() {").unwrap();
        lines[12] = "    changed_first();".into();
        lines[second + 12] = "    changed_second();".into();
        let rhs = lines.join("\n") + "\n";
        let review = DiffResult::from_sources("a.rs", &lhs, &rhs);
        assert_eq!(review.hunks.len(), 2);
        for (i, hunk) in review.hunks.iter().enumerate() {
            let mut selected = LineSelection::default();
            selected.include_context(&hunk.context);
            let (own, other) = if i == 0 { (0, second) } else { (second, 0) };
            assert!(selected.rhs.contains(&own));
            assert!(!selected.rhs.contains(&other));
        }
        let domain = review.domain_json();
        assert!(domain.get("context").is_none());
        assert!(domain["hunks"][0]["lines"].is_array());
        assert!(domain["hunks"][0]["context"].is_array());
    }

    #[test]
    fn consecutive_signature_context_is_one_range() {
        let mut body = String::new();
        for i in 0..30 {
            writeln!(body, "    value_{i} = {i}").unwrap();
        }
        let lhs = format!("def run(\n    first,\n    second,\n):\n{body}");
        let rhs = lhs.replace("value_20 = 20", "value_20 = 999");
        let diff = DiffResult::from_sources("a.py", &lhs, &rhs);
        assert!(diff.hunks.iter().flat_map(|h| &h.context).any(|r| matches!(r,
            ContextRange { lhs, rhs } if lhs.start.line.as_usize() == 0 && lhs.end.line.as_usize() == 3 && rhs.end.line.as_usize() == 3)));
    }

    #[test]
    fn unrelated_tail_return_is_not_extra_context() {
        let lhs = include_str!("../../examples/review/real/07-ripgrep-3487/before.rs");
        let rhs = include_str!("../../examples/review/real/07-ripgrep-3487/after.rs");
        let review = DiffResult::from_sources("a.rs", lhs, rhs);
        assert!(!review.snapshot().contains("Ok(if matched"));
        assert!(review.snapshot().contains("fn run("));
    }

    #[test]
    fn changed_entry_keeps_enclosing_return_boundaries() {
        let mut entries = String::new();
        for i in 0..24 {
            writeln!(entries, "        '{i}': {i},").unwrap();
        }
        let lhs = format!("def values():\n    return {{\n{entries}    }}\n");
        let rhs = lhs.replace("'12': 12", "'12': 999");
        let review = DiffResult::from_sources("a.py", &lhs, &rhs);
        let mut context = LineSelection::default();
        for hunk in &review.hunks {
            context.include_context(&hunk.context);
        }
        assert!(
            context.rhs.contains(&1),
            "return opener is beyond ordinary padding"
        );
        assert!(
            context.rhs.contains(&26),
            "returned dictionary closer is beyond padding"
        );
    }
}
mod hunk_tests {
    use super::text;
    use crate::display::line_layout as layout;
    use crate::parse::folds::Correspondence;
    use crate::summary::DiffResult;
    use crate::summary::FileFormat;
    #[test]
    fn context_overlap_merges_transitively_without_filling_the_gap() {
        let make = |line: u32, context: &[usize]| {
            let mut hunk = crate::display::hunks::Hunk {
                novel_lhs: [line_numbers::LineNumber::from(line)].into_iter().collect(),
                novel_rhs: [line_numbers::LineNumber::from(line)].into_iter().collect(),
                lines: vec![(Some(line.into()), Some(line.into()))],
                context: Vec::new(),
            };
            hunk.context = context
                .iter()
                .map(|&line| crate::display::hunks::ContextRange {
                    lhs: crate::lines::SourceRange::line("header", line),
                    rhs: crate::lines::SourceRange::line("header", line),
                })
                .collect();
            hunk
        };
        let hunks = [make(10, &[0]), make(40, &[0, 90]), make(70, &[90])];
        let mut merged = crate::display::hunks::merge_adjacent(
            &hunks,
            &Default::default(),
            &Default::default(),
            100.into(),
            100.into(),
            3,
        );
        crate::display::syntax_context::compact_hunk_context(&mut merged);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].lines.len(), 3);
        assert_eq!(merged[0].context.len(), 2);
    }

    #[test]
    fn unsupported_language_uses_text_diff_without_syntax_annotations() {
        let result = DiffResult::from_sources("a.txt", "hello old\n", "hello new\n");
        assert!(matches!(result.file_format, FileFormat::PlainText));
        assert!(result.folds.is_empty() && result.hunks.iter().all(|h| h.context.is_empty()));
        assert!(result.snapshot().contains("- hello old"));
        assert!(result.snapshot().contains("+ hello new"));
    }

    #[test]
    fn real_annotations_have_valid_ranges_and_unchanged_context() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/review/real");
        for dir in std::fs::read_dir(root).unwrap() {
            let dir = dir.unwrap().path();
            let p: serde_json::Value =
                serde_json::from_slice(&std::fs::read(dir.join("provenance.json")).unwrap())
                    .unwrap();
            let read_source = |side: &str| {
                std::fs::read_to_string(dir.join(p["sources"][side]["file"].as_str().unwrap()))
                    .unwrap()
            };
            let (lhs_src, rhs_src) = (read_source("lhs"), read_source("rhs"));
            let path = p["sources"]["rhs"]["path"].as_str().unwrap();
            let result = DiffResult::from_sources(path, &lhs_src, &rhs_src);
            let positions = (&result.lhs_positions[..], &result.rhs_positions[..]);
            let baseline = layout::baseline(
                positions,
                &layout::aligned_rows(layout::sources(&result), positions),
            );
            let lhs_novel = layout::novel_lines(&result.lhs_positions);
            let rhs_novel = layout::novel_lines(&result.rhs_positions);
            assert!(lhs_novel.is_subset(&baseline.lhs));
            assert!(rhs_novel.is_subset(&baseline.rhs));
            for fold in &result.folds {
                match &fold.regions {
                    Correspondence::Paired { lhs, rhs } => {
                        text(&lhs_src, lhs);
                        text(&rhs_src, rhs);
                    }
                    Correspondence::Added(rhs) => {
                        text(&rhs_src, rhs);
                    }
                    Correspondence::Deleted(lhs) => {
                        text(&lhs_src, lhs);
                    }
                }
            }
            let mut seen = std::collections::BTreeSet::new();
            for context in result.hunks.iter().flat_map(|h| &h.context) {
                text(&lhs_src, &context.lhs);
                text(&rhs_src, &context.rhs);
                assert!(context.lhs.rows().all(|line| !lhs_novel.contains(&line)));
                assert!(context.rhs.rows().all(|line| !rhs_novel.contains(&line)));
                for row in context.lhs.rows().zip(context.rhs.rows()) {
                    assert!(
                        seen.insert(row),
                        "duplicate context across hunks: {}",
                        dir.display()
                    );
                }
            }
        }
    }

    #[test]
    fn byte_limit_fallback_has_no_syntax_annotations() {
        let file = crate::options::FileArgument::NamedPath("a.py".into());
        let options = crate::options::DiffOptions {
            byte_limit: 1,
            ..Default::default()
        };
        let diff = crate::diff_file_content(
            "a.py",
            None,
            &file,
            &file,
            "x = 1\n",
            "x = 2\n",
            &Default::default(),
            &options,
            &[],
        );
        assert!(matches!(diff.file_format, FileFormat::TextFallback { .. }));
        assert!(diff.folds.is_empty());
        assert!(diff.hunks.iter().all(|hunk| hunk.context.is_empty()));
        let result = diff;
        assert!(result.snapshot().contains("+ x = 2"));
    }
}
