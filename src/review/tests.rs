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
    use crate::parse::folds::{Fold, FoldMatch};
    use crate::summary::DiffResult;
    use std::fmt::Write as _;

    fn paired(fold: &Fold) -> Option<(&SourceRange, &SourceRange)> {
        match &fold.match_kind {
            FoldMatch::Unchanged { opposite } => Some((&fold.range, opposite)),
            FoldMatch::Novel => None,
        }
    }
    fn added(fold: &Fold) -> Option<&SourceRange> {
        matches!(fold.match_kind, FoldMatch::Novel).then_some(&fold.range)
    }

    #[test]
    fn renamed_function_with_changed_body_uses_structural_correspondence() {
        let lhs = "fn run() {\n    old_work();\n}\n";
        let rhs = "fn execute() {\n    old_work();\n    new_work();\n}\n";
        let diff = DiffResult::from_sources("a.rs", lhs, rhs);
        assert!(diff.lhs_folds.iter().any(|f| {
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
        assert_eq!(diff.rhs_folds.len(), 1);
    }

    #[test]
    fn multiline_atoms_and_test_bodies_keep_fold_metadata() {
        let src = "def test_read():\n    value = \"\"\"first\nsecond\"\"\"\n    return value\n";
        let result = DiffResult::from_sources("a.py", "", src);
        assert!(result
            .rhs_folds
            .iter()
            .any(|fold| fold.tags.iter().any(|tag| tag == "test")));
        assert!(result
            .rhs_folds
            .iter()
            .any(|fold| fold.placeholder == "String"
                && added(fold).is_some_and(|r| text(src, r) == "\"\"\"first\nsecond\"\"\"")));
    }

    #[test]
    fn cfg_test_modules_are_test_modules_and_plain_modules_are_not() {
        let src = "mod plain {\n    fn a() {}\n}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {\n        a();\n    }\n}\n";
        let result = DiffResult::from_sources("a.rs", "", src);
        let tags: Vec<Vec<String>> = result
            .rhs_folds
            .iter()
            .filter(|fold| fold.tags.iter().any(|tag| tag == "module"))
            .map(|fold| fold.tags.clone())
            .collect();
        assert_eq!(
            tags,
            [vec!["body", "module"], vec!["body", "module", "test"]]
        );
        assert!(result
            .rhs_folds
            .iter()
            .any(|fold| fold.tags == ["body", "function", "test"]));
    }

    #[test]
    fn flattened_test_body_keeps_the_existing_string_match_on_both_sides() {
        let lhs = "def test_doc():\n    \"\"\"some shared words before\"\"\"\n";
        let rhs = "def test_doc():\n    \"\"\"some shared words after\"\"\"\n";
        let result = DiffResult::from_sources("a.py", lhs, rhs);
        assert_eq!(result.lhs_folds.len(), 1);
        assert_eq!(result.rhs_folds.len(), 1);
        let fold = &result.lhs_folds[0];
        assert_eq!(fold.tags, ["body", "function", "test"]);
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
            for (folds, own, opposite) in [(&diff.lhs_folds, lhs, rhs), (&diff.rhs_folds, rhs, lhs)]
            {
                if own.is_empty() {
                    assert!(folds.is_empty());
                    continue;
                }
                assert_eq!(folds.len(), 2);
                for (fold, expected) in folds.iter().zip(["import os", "import sys"]) {
                    assert_eq!(fold.tags, ["import"]);
                    assert_eq!(text(own, &fold.range), expected);
                    match &fold.match_kind {
                        FoldMatch::Unchanged { opposite: range } => {
                            assert_eq!(text(opposite, range), expected)
                        }
                        FoldMatch::Novel => assert!(opposite.is_empty()),
                    }
                }
            }
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
            .lhs_folds
            .iter()
            .any(|f| matches!(f.placeholder.as_str(), "Import" | "Imports")));
    }

    #[test]
    fn import_folds_remain_one_sided_when_file_added_or_deleted() {
        let source = "import os\n\ndef f():\n    return os.getcwd()\n";
        let added_diff = DiffResult::from_sources("a.py", "", source);
        assert!(added_diff.lhs_folds.is_empty());
        assert_eq!(
            text(source, added(&added_diff.rhs_folds[0]).unwrap()),
            "import os"
        );
        let deleted_diff = DiffResult::from_sources("a.py", source, "");
        assert!(deleted_diff.rhs_folds.is_empty());
        assert_eq!(
            text(source, added(&deleted_diff.lhs_folds[0]).unwrap()),
            "import os"
        );
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
                review.lhs_folds.iter().any(|f| {
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
        assert!(review.rhs_folds.iter().any(|f| {
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
                    .rhs_folds
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
        assert!(review.lhs_folds.iter().any(|f| {
            paired(f).is_some_and(|(l, r)| {
                text(lhs, l) == "\"☕\", oldValue" && text(rhs, r) == "\"☕\", newValue"
            })
        }));
    }
}

mod syntax_tests {
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
        let prepared = &diff;
        assert_eq!(prepared.hunks.len(), 1);
        let mut seen = std::collections::BTreeSet::new();
        for (lhs, rhs) in &prepared.hunks[0].lines {
            let pair = (lhs.unwrap().as_usize(), rhs.unwrap().as_usize());
            assert!(seen.insert(pair), "duplicate displayed row");
        }
        assert!(!seen.contains(&(24, 24)), "distant gaps stay hidden");
        assert!(seen.contains(&(0, 0)));
        assert!(seen.contains(&(71, 71)));
    }

    #[test]
    fn new_and_deleted_functions_have_folds_but_no_extra_context() {
        let source = "fn run() {\n    work();\n}\n";
        for (lhs, rhs) in [("", source), (source, "")] {
            let diff = DiffResult::from_sources("a.rs", lhs, rhs);
            assert!(!diff.lhs_folds.is_empty() || !diff.rhs_folds.is_empty());
            let prepared = &diff;
            assert!(prepared
                .hunks
                .iter()
                .flat_map(|h| &h.lines)
                .all(|(lhs, rhs)| lhs.is_none() || rhs.is_none()));
        }
    }

    #[test]
    fn unicode_crlf_and_multiline_return_boundaries() {
        let lhs =
        "def café():\r\n    x = 1\r\n    return (\r\n        x,\r\n\r\n        '☕',\r\n    )\r\n";
        let rhs = lhs.replace("x = 1", "x = 2");
        let result = DiffResult::from_sources("a.py", lhs, &rhs);
        assert!(matches!(&result.rhs_src,FileContent::Text(s) if s==&rhs));
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
        let lhs = format!(
            "{}{}{}",
            function("first"),
            "\n".repeat(12),
            function("second")
        );
        let mut lines: Vec<_> = lhs.lines().map(str::to_owned).collect();
        let second = lines.iter().position(|l| l == "fn second() {").unwrap();
        lines[12] = "    changed_first();".into();
        lines[second + 12] = "    changed_second();".into();
        let rhs = lines.join("\n") + "\n";
        let review = DiffResult::from_sources("a.rs", &lhs, &rhs);
        let prepared = &review;
        assert_eq!(prepared.hunks.len(), 2);
        for (i, hunk) in prepared.hunks.iter().enumerate() {
            let selected: std::collections::BTreeSet<_> = hunk
                .lines
                .iter()
                .filter_map(|(_, rhs)| rhs.map(|line| line.as_usize()))
                .collect();
            let (own, other) = if i == 0 { (0, second) } else { (second, 0) };
            assert!(selected.contains(&own));
            assert!(!selected.contains(&other));
        }
        let domain = review.domain_json();
        assert!(domain.get("context").is_none());
        assert!(domain["hunks"][0]["lines"].is_array());
        assert!(domain["hunks"][0].get("context").is_none());
    }

    #[test]
    fn generator_declarations_keep_enclosing_signature_context() {
        let body = (0..30)
            .map(|i| format!("    yield value_{i};\n"))
            .collect::<String>();
        for extension in ["js", "jsx", "ts", "tsx"] {
            for prefix in ["export function*", "export async function*"] {
                let lhs = format!("{prefix} stream(\n    chunks,\n) {{\n{body}}}\n");
                let rhs = lhs.replace("yield value_20;", "yield changed_value;");
                let diff = DiffResult::from_sources(&format!("stream.{extension}"), &lhs, &rhs);
                let selected = super::selection_without_padding(&diff);
                assert!(
                    (0..3).all(|line| selected.lhs.contains(&line) && selected.rhs.contains(&line)),
                    "missing generator signature: {extension}, {prefix}"
                );
                assert!(
                    selected.lhs.contains(&33) && selected.rhs.contains(&33),
                    "missing closing brace: {extension}, {prefix}"
                );
                assert!(
                    !selected.lhs.contains(&10) && !selected.rhs.contains(&10),
                    "unrelated body should stay hidden"
                );
            }
        }
    }

    #[test]
    fn consecutive_signature_context_is_selected() {
        let mut body = String::new();
        for i in 0..30 {
            writeln!(body, "    value_{i} = {i}").unwrap();
        }
        let lhs = format!("def run(\n    first,\n    second,\n):\n{body}");
        let rhs = lhs.replace("value_20 = 20", "value_20 = 999");
        let diff = DiffResult::from_sources("a.py", &lhs, &rhs);
        let selected = super::selection_without_padding(&diff);
        assert!((0..4).all(|line| selected.lhs.contains(&line) && selected.rhs.contains(&line)));
    }

    #[test]
    fn signature_context_does_not_expose_neighboring_statements() {
        let lhs = include_str!("../../examples/review/real/02-review-175/before.ts");
        let rhs = include_str!("../../examples/review/real/02-review-175/after.ts");
        let diff = DiffResult::from_sources("a.ts", lhs, rhs);
        let output = diff.snapshot();
        assert!(output.contains("function parseReviewDiffFile("));
        assert!(!output.contains("return sections.filter("));
        assert!(!output.contains("const lines = section.split("));
        assert!(
            output.contains("let additions = 0;"),
            "changes retain ordinary context"
        );
    }

    #[test]
    fn unrelated_tail_return_is_not_extra_context() {
        let lhs = include_str!("../../examples/review/real/07-ripgrep-3487/before.rs");
        let rhs = include_str!("../../examples/review/real/07-ripgrep-3487/after.rs");
        let review = DiffResult::from_sources("a.rs", lhs, rhs);
        let selected = super::selection_without_padding(&review);
        let return_line = rhs
            .lines()
            .position(|line| line.contains("Ok(if matched"))
            .unwrap();
        assert!(
            !selected.rhs.contains(&return_line),
            "the unrelated return is not syntax context"
        );
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
        let context = super::selection_without_padding(&review);
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
    use crate::parse::folds::FoldMatch;
    use crate::summary::DiffResult;
    use crate::summary::FileFormat;
    #[test]
    fn touching_windows_merge_but_a_hidden_row_separates_hunks() {
        use crate::display::hunks::Hunk;
        use line_numbers::LineNumber;
        use std::fmt::Write;
        let mut lhs = String::new();
        for line in 0..40 {
            writeln!(lhs, "line {line}").unwrap();
        }
        for (second, expected_hunks) in [(17, 1), (18, 2)] {
            let rhs = lhs
                .replace("line 10\n", "changed 10\n")
                .replace(&format!("line {second}\n"), &format!("changed {second}\n"));
            let diff = DiffResult::from_sources("a.txt", &lhs, &rhs);
            // Give preparation the two separate change seeds: upstream may have
            // already merged them using its different padding policy.
            let raw: Vec<_> = [10, second]
                .into_iter()
                .map(|line| {
                    let line = LineNumber(line);
                    Hunk {
                        novel_lhs: [line].into_iter().collect(),
                        novel_rhs: [line].into_iter().collect(),
                        lines: vec![(Some(line), Some(line))],
                    }
                })
                .collect();
            let hunks = crate::display::prepare::prepare(
                &raw,
                (&lhs, &rhs),
                (&diff.lhs_positions, &diff.rhs_positions),
                &Default::default(),
                3,
            );
            assert_eq!(hunks.len(), expected_hunks);
            let selected = layout::LineSelection::from_hunks(&hunks);
            assert_eq!(selected.lhs.contains(&14), second == 17);
        }
    }

    #[test]
    fn identical_files_keep_alignment_for_showing_hidden_source() {
        let source = "fn unchanged() {\n    work();\n}\n";
        let diff = DiffResult::from_sources("a.rs", source, source);
        assert!(diff.hunks.is_empty());
        let viewer = diff.viewer_json();
        assert_eq!(
            viewer["layout"]["rows"],
            serde_json::json!([[0, 0], [1, 1], [2, 2]])
        );
    }

    #[test]
    fn unsupported_language_uses_text_diff_without_syntax_annotations() {
        let result = DiffResult::from_sources("a.txt", "hello old\n", "hello new\n");
        assert!(matches!(result.file_format, FileFormat::PlainText));
        assert!(result.lhs_folds.is_empty() && result.rhs_folds.is_empty());
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
            let prepared = &result;
            let baseline = layout::LineSelection::from_hunks(&prepared.hunks);
            let lhs_novel = layout::novel_lines(&result.lhs_positions);
            let rhs_novel = layout::novel_lines(&result.rhs_positions);
            assert!(lhs_novel.is_subset(&baseline.lhs));
            assert!(rhs_novel.is_subset(&baseline.rhs));
            for (folds, own_src, opposite_src, opposite_folds) in [
                (&result.lhs_folds, &lhs_src, &rhs_src, &result.rhs_folds),
                (&result.rhs_folds, &rhs_src, &lhs_src, &result.lhs_folds),
            ] {
                for fold in folds {
                    text(own_src, &fold.range);
                    if let FoldMatch::Unchanged { opposite } = &fold.match_kind {
                        text(opposite_src, opposite);
                        assert!(opposite_folds.iter().any(|other| {
                            other.range == *opposite
                                && matches!(&other.match_kind, FoldMatch::Unchanged { opposite: back } if *back == fold.range)
                        }), "matched folds must be reciprocal");
                    }
                }
            }
            let mut seen = std::collections::BTreeSet::new();
            for row in prepared.hunks.iter().flat_map(|h| &h.lines) {
                assert!(
                    seen.insert(*row),
                    "duplicate selected row: {}",
                    dir.display()
                );
                if let Some(lhs) = row.0 {
                    assert!(lhs.as_usize() < lhs_src.lines().count());
                }
                if let Some(rhs) = row.1 {
                    assert!(rhs.as_usize() < rhs_src.lines().count());
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
            &crate::config::Params::default(),
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
        assert!(diff.lhs_folds.is_empty() && diff.rhs_folds.is_empty());
        let result = diff;
        assert!(result.snapshot().contains("+ x = 2"));
    }
}

fn selection_without_padding(
    diff: &crate::summary::DiffResult,
) -> crate::display::line_layout::LineSelection {
    let (lhs, rhs) = crate::display::line_layout::sources(diff);
    let file = crate::options::FileArgument::NamedPath(diff.display_path.clone().into());
    let options = crate::options::DisplayOptions {
        num_context_lines: 0,
        ..Default::default()
    };
    let diff = crate::diff_file_content(
        &crate::config::Params::default(),
        &diff.display_path,
        None,
        &file,
        &file,
        lhs,
        rhs,
        &options,
        &crate::options::DiffOptions::default(),
        &[],
    );
    crate::display::line_layout::LineSelection::from_hunks(&diff.hunks)
}
