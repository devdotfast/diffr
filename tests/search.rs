//! End-to-end tests for `diffr search` and its steps.
mod git_fixture;
mod support;

use gix::Repository;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
};
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    repo: Repository,
    base: String,
    head: String,
}
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = gix::init(dir.path().join("repo")).unwrap();
        git_fixture::git(
            repo.workdir().unwrap(),
            &["config", "core.autocrlf", "false"],
        );
        fs::create_dir_all(dir.path().join("config/diffr")).unwrap();
        let mut fixture = Self {
            dir,
            repo,
            base: String::new(),
            head: String::new(),
        };
        fixture.copy("base");
        fixture.base = fixture.commit();
        for entry in fs::read_dir(fixture.root()).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                fs::remove_file(path).unwrap();
            }
        }
        fixture.copy("head");
        fixture.head = fixture.commit();
        fixture
    }
    fn root(&self) -> PathBuf {
        self.dir.path().join("repo")
    }
    fn copy(&self, side: &str) {
        let fixtures = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/search")
            .join(side);
        for entry in fs::read_dir(fixtures).unwrap() {
            let entry = entry.unwrap();
            fs::copy(entry.path(), self.root().join(entry.file_name())).unwrap();
        }
    }
    fn commit(&self) -> String {
        git_fixture::commit(&self.repo, "search fixture")
    }
    fn grep(&self, null: bool, path: Option<&str>) -> Vec<u8> {
        let mut command = Command::new("git");
        command
            .current_dir(self.root())
            .env_remove("GIT_DIR")
            .args([
                "--no-pager",
                "grep",
                "--no-color",
                "--no-heading",
                "--no-break",
                "--full-name",
                "-n",
                "-F",
            ]);
        if null {
            command.arg("-z");
        }
        command.args(["-e", "search_token", &self.base, &self.head, "--"]);
        if let Some(path) = path {
            command.arg(path);
        }
        let output = command.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }
    fn run(&self, args: &[&str], input: &[u8]) -> Output {
        let mut child = support::get_base_command()
            .args(args)
            .current_dir(if !matches!(args, ["search", "pprint", ..]) {
                self.root()
            } else {
                self.dir.path().to_path_buf()
            })
            .env("XDG_CONFIG_HOME", self.dir.path().join("config"))
            .env_remove("GIT_DIR")
            .env_remove("GEMINI_API_KEY")
            .env_remove("GOOGLE_API_KEY")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        // Write concurrently: a rejected invocation can exit without reading stdin.
        let mut stdin = child.stdin.take().unwrap();
        let bytes = input.to_vec();
        let writer = std::thread::spawn(move || {
            let _ = stdin.write_all(&bytes);
        });
        let output = child.wait_with_output().unwrap();
        writer.join().unwrap();
        output
    }
    fn search(&self, input: &[u8]) -> Value {
        value(self.run(&["search", &self.base, &self.head], input))
    }
    fn json(&self, args: &[&str], input: &Value) -> Output {
        self.run(args, &serde_json::to_vec(input).unwrap())
    }
}
fn success(output: Output) -> Vec<u8> {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}
fn value(output: Output) -> Value {
    serde_json::from_slice(&success(output)).unwrap()
}
fn failure(output: Output, message: &str) {
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(message),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
fn find<'a>(results: &'a Value, side: &str, path: &str) -> &'a Value {
    results
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["file"][side]["path"] == path)
        .unwrap()
}
fn walk(regions: &mut Value, visit: &mut impl FnMut(&mut Value)) {
    for region in regions.as_array_mut().unwrap() {
        visit(region);
        if region["kind"] == "fold" {
            walk(&mut region["children"], visit);
        }
    }
}

#[test]
fn real_git_grep_hydrates_both_commits_and_deleted_files_without_worktrees() {
    let fixture = Fixture::new();
    let normal = fixture.search(&fixture.grep(false, None));
    assert_eq!(normal, fixture.search(&fixture.grep(true, None)));
    let retry = find(&normal, "rhs", "retry.js");
    assert!(retry["diff"]["lhs"]["text"]
        .as_str()
        .unwrap()
        .contains("const attempts = 3;"));
    assert!(retry["diff"]["rhs"]["text"]
        .as_str()
        .unwrap()
        .contains("options.attempts"));
    assert!(find(&normal, "lhs", "removed.js")["diff"]["rhs"].is_null());
    assert!(find(&normal, "rhs", "added.js")["diff"]["lhs"].is_null());
    assert_eq!(fixture.search(b""), json!([]));
}

#[test]
fn validates_hit_origin_coordinates_and_full_line_text() {
    let fixture = Fixture::new();
    for (hit, message) in [
        ("HEAD:retry.js:1:stale\n", "hit text differs"),
        ("HEAD:retry.js:0:\n", "1-based"),
        ("HEAD:retry.js:999:\n", "out of bounds"),
        ("HEAD:../retry.js:1:text\n", "repository-relative"),
        ("HEAD:/retry.js:1:text\n", "repository-relative"),
        ("HEAD:missing.js:1:text\n", "file in that commit"),
        ("retry.js:1:text\n", "revision:path:line:text"),
    ] {
        failure(
            fixture.run(&["search", &fixture.base, &fixture.head], hit.as_bytes()),
            message,
        );
    }
    fs::write(fixture.root().join("third.js"), "third commit").unwrap();
    fixture.commit();
    failure(
        fixture.run(
            &["search", &fixture.base, &fixture.head],
            b"HEAD:retry.js:1:text\n",
        ),
        "outside scoped commits",
    );
    failure(
        fixture.run(&["search", "not-a-revision", &fixture.head], b""),
        "revision",
    );
}

#[test]
fn plugin_overrides_replace_the_search_plugins() {
    let fixture = Fixture::new();
    let options = fixture.root().join("options.json");
    fs::write(&options, r#"{"plugins":{"order":[]}}"#).unwrap();
    let processed = value(fixture.run(
        &[
            "search",
            &fixture.base,
            &fixture.head,
            "--options",
            options.to_str().unwrap(),
        ],
        &fixture.grep(true, Some("retry.js")),
    ));
    let preview =
        String::from_utf8(success(fixture.json(&["search", "pprint"], &processed))).unwrap();
    assert!(preview.contains("const response = request();"));
    assert!(!preview.contains("collapsed"));
}

#[test]
fn pprint_rejects_results_whose_file_and_sides_disagree() {
    let fixture = Fixture::new();
    let mut incomplete = fixture.search(&fixture.grep(true, Some("retry.js")));
    incomplete[0]["file"].as_object_mut().unwrap().remove("lhs");
    failure(
        fixture.json(&["search", "pprint"], &incomplete),
        "file and source sides",
    );
}

#[test]
fn pprint_expands_saved_folds_without_repository_or_configuration_access() {
    let fixture = Fixture::new();
    let mut selected = fixture.search(&fixture.grep(true, Some("retry.js")));
    let before =
        String::from_utf8(success(fixture.json(&["search", "pprint"], &selected))).unwrap();
    assert!(!before.contains("const response = request();"));
    for side in ["lhs", "rhs"] {
        walk(
            &mut selected[0]["diff"][side]["root"]["children"],
            &mut |r| {
                r["visibility"]["collapsed"] = json!(false);
            },
        );
    }
    selected[0]["scope"]["repo"] = json!(fixture.dir.path().join("no-repository"));
    fs::write(
        fixture.dir.path().join("config/diffr/config.toml"),
        "invalid [ config",
    )
    .unwrap();
    let after = String::from_utf8(success(fixture.json(&["search", "pprint"], &selected))).unwrap();
    assert!(after.contains("const response = request();"));
    assert!(!after.contains("collapsed"));
    assert!(after.contains("const attempts = 3;"));
    assert!(after.contains("options.attempts"));
}

#[test]
fn rename_correspondence_pairs_old_and_new_paths() {
    let mut fixture = Fixture::new();
    fs::rename(
        fixture.root().join("retry.js"),
        fixture.root().join("renamed.js"),
    )
    .unwrap();
    fixture.head = fixture.commit();
    let results = fixture.search(&fixture.grep(true, None));
    let result = find(&results, "rhs", "renamed.js");
    assert_eq!(result["file"]["lhs"]["path"], "retry.js");
    let preview = String::from_utf8(success(
        fixture.json(&["search", "pprint"], &json!([result])),
    ))
    .unwrap();
    assert!(preview.contains("retry.js → renamed.js"));
}

#[test]
fn aliases_crlf_and_utf8_preserve_blob_text_and_byte_highlights() {
    let mut fixture = Fixture::new();
    let text = "const café = 'search_token: 🦀';\r\n";
    fs::write(fixture.root().join("unicode.js"), text).unwrap();
    fixture.head = fixture.commit();
    let hit = format!("HEAD:unicode.js:1:{text}");
    let selected = value(fixture.run(&["search", "HEAD~2", "HEAD"], hit.as_bytes()));
    assert_eq!(selected[0]["scope"]["base"], fixture.base);
    assert_eq!(selected[0]["scope"]["head"], fixture.head);
    assert_eq!(selected[0]["diff"]["rhs"]["text"], text);
    let mut source = selected[0]["diff"]["rhs"].clone();
    let mut spans = Vec::new();
    walk(&mut source["root"]["children"], &mut |r| {
        if let Some(highlights) = r["search_highlights"].as_array() {
            spans.extend(highlights.clone());
        }
    });
    assert_eq!(
        spans,
        vec![
            json!({"line":0,"start_column":0,"end_column":text.trim_end_matches(['\r','\n']).len()})
        ]
    );
}

#[cfg(unix)]
#[test]
fn nul_framed_git_grep_handles_ambiguous_filenames() {
    let mut fixture = Fixture::new();
    let name = "odd:12:name\n.js";
    fs::write(fixture.root().join(name), "const token = 'search_token';\n").unwrap();
    fixture.head = fixture.commit();
    let selected = fixture.search(&fixture.grep(true, Some(name)));
    assert_eq!(selected[0]["file"]["rhs"]["path"], name);
    assert_eq!(
        selected[0]["diff"]["rhs"]["text"],
        "const token = 'search_token';\n"
    );
}

#[test]
fn shared_unchanged_content_keeps_search_context_and_collapses_distant_lines() {
    let mut fixture = Fixture::new();
    let before: String = (0..20)
        .map(|i| format!("  const before{i} = {i};\n"))
        .collect();
    let after: String = (0..20)
        .map(|i| format!("  const after{i} = {i};\n"))
        .collect();
    fs::write(
        fixture.root().join("long.js"),
        format!("function unchanged() {{\n{before}  const search_token = 42;\n{after}}}\n"),
    )
    .unwrap();
    fixture.base = fixture.commit();
    fixture.head = fixture.commit();
    let processed = fixture.search(&fixture.grep(true, Some("long.js")));
    let rendered =
        String::from_utf8(success(fixture.json(&["search", "pprint"], &processed))).unwrap();
    assert!(rendered.contains("function unchanged()"));
    assert!(rendered.contains("const search_token = 42;"));
    assert!(!rendered.contains("const before5 = 5;"));
    assert!(!rendered.contains("const after15 = 15;"));
}

#[test]
fn search_defaults_use_highlights_and_keep_the_opposite_edit_and_complete_header() {
    let mut fixture = Fixture::new();
    let header = "fn visitor(\n    value: u32,\n) {\n";
    let body: String = (0..20).map(|i| format!("    let a{i} = {i};\n")).collect();
    let path = fixture.root().join("visitor.rs");
    fs::write(
        &path,
        format!("const UNRELATED: u32 = 1;\n{header}{body}    old_reference();\n}}\n"),
    )
    .unwrap();
    fixture.base = fixture.commit();
    fs::write(
        &path,
        format!("const UNRELATED: u32 = 2;\n{header}{body}    search_token();\n}}\n"),
    )
    .unwrap();
    fixture.head = fixture.commit();
    fs::write(
        fixture.dir.path().join("config/diffr/config.toml"),
        "[plugins.shape.bundled.context]\nlines = 1\n",
    )
    .unwrap();
    let processed = fixture.search(&fixture.grep(true, Some("visitor.rs")));
    let text = String::from_utf8(success(fixture.json(&["search", "pprint"], &processed))).unwrap();
    for line in header.lines() {
        assert!(text.contains(line), "missing {line}:\n{text}");
    }
    assert!(
        text.contains("old_reference();"),
        "opposite edit missing:\n{text}"
    );
    assert!(text.contains("search_token();"));
    assert!(text.contains("let a19 = 19;"));
    assert!(
        !text.contains("let a18 = 18;"),
        "configured width must be one:\n{text}"
    );
    assert!(
        !text.contains("const UNRELATED"),
        "unrelated edit must fold:\n{text}"
    );
}

#[test]
fn documented_rust_results_keep_headers_and_native_relations_through_json_and_cache() {
    let mut fixture = Fixture::new();
    let header = "/// Visit each region.\n/// Keep its context.\nfn visit(\n    regions: &[Region],\n    rhs: &OtherSide,\n    threshold: usize,\n    under_collapsed: bool,\n    gates: Gates,\n    leaves: &mut Vec<(u32, u32)>,\n) {\n    for region in regions {\n";
    let body: String = (0..20)
        .map(|i| format!("        let a{i} = {i};\n"))
        .collect();
    let path = fixture.root().join("visitor.rs");
    fs::write(
        &path,
        format!("{header}{body}        search_token(false);\n    }}\n}}\n"),
    )
    .unwrap();
    fixture.base = fixture.commit();
    fs::write(
        &path,
        format!("{header}{body}        search_token(true);\n    }}\n}}\n"),
    )
    .unwrap();
    fixture.head = fixture.commit();
    let hits = fixture.grep(true, Some("visitor.rs"));
    // The first search computes the diff; the second reads the cache.
    let processed = fixture.search(&hits);
    assert_eq!(processed, fixture.search(&hits));
    let text = String::from_utf8(success(fixture.json(&["search", "pprint"], &processed))).unwrap();
    for line in header.lines() {
        assert!(text.contains(line), "missing {line}:\n{text}");
    }
    assert!(!text.contains("let a5 = 5;"));
}
