//! The plugins on the stream: hidden files, collapsed and linked regions, and
//! a run cut short by a failing plugin.
mod support;

use git2::{IndexAddOption, Repository, Signature, Time};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::Output;
use support::get_base_command;
use tempfile::TempDir;

struct Fixture {
    dir: TempDir,
    repo: Repository,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path().join("repo")).unwrap();
        fs::create_dir_all(dir.path().join("config/diffr")).unwrap();
        Self { dir, repo }
    }

    fn write(&self, path: &str, text: &str) {
        let path = self.dir.path().join("repo").join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    fn remove(&self, path: &str) {
        fs::remove_file(self.dir.path().join("repo").join(path)).unwrap();
    }

    /// A file next to the global configuration file.
    fn config_file(&self, path: &str, text: &str) -> PathBuf {
        let path = self.dir.path().join("config/diffr").join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    fn commit(&self) -> String {
        let mut index = self.repo.index().unwrap();
        index.add_all(["*"], IndexAddOption::DEFAULT, None).unwrap();
        index.update_all(["*"], None).unwrap();
        index.write().unwrap();
        let tree = self.repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = Signature::new(
            "Fixture",
            "fixture@example.invalid",
            &Time::new(946684800, 0),
        )
        .unwrap();
        let parent = self
            .repo
            .head()
            .ok()
            .map(|head| head.peel_to_commit().unwrap());
        let parents: Vec<_> = parent.iter().collect();
        self.repo
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                "fixture\n",
                &tree,
                &parents,
            )
            .unwrap()
            .to_string()
    }

    fn run(&self, base: &str, head: &str) -> Output {
        self.diffr(&[base, head, "--format", "ndjson"])
    }

    fn diffr(&self, args: &[&str]) -> Output {
        get_base_command()
            .arg("--repo")
            .arg(self.dir.path().join("repo"))
            .args(args)
            .env("XDG_CONFIG_HOME", self.dir.path().join("config"))
            .env_remove("GIT_DIR")
            .env_remove("GEMINI_API_KEY")
            .env_remove("GOOGLE_API_KEY")
            .output()
            .unwrap()
    }
}

fn records(output: &Output) -> Vec<Value> {
    String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The `file` record for a path.
fn record<'a>(records: &'a [Value], path: &str) -> &'a Value {
    records
        .iter()
        .find(|record| {
            record["type"] == "file"
                && ["lhs", "rhs"]
                    .iter()
                    .any(|side| record["file"][side]["path"] == path)
        })
        .unwrap_or_else(|| panic!("no record for {path}"))
}

/// Every region on one side of a file record, parents first.
fn regions(record: &Value, side: &str) -> Vec<Value> {
    fn walk(regions: &Value, out: &mut Vec<Value>) {
        for region in regions.as_array().into_iter().flatten() {
            out.push(region.clone());
            walk(&region["children"], out);
        }
    }
    let mut out = Vec::new();
    walk(&record["diff"][side]["regions"], &mut out);
    out
}

const GONE: &str = "/// Gone for good.\n/// Really.\nfn gone() -> u32 {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n    let d = 4;\n    let e = 5;\n    let f = 6;\n    let g = 7;\n    let h = 8;\n    let i = 9;\n    let j = 10;\n    let k = 11;\n    a + b + c + d + e + f + g + h + i + j + k\n}\n";

#[test]
fn the_default_view_hides_links_and_collapses() {
    let fixture = Fixture::new();
    fixture.write("src/lib.rs", &format!("fn keep() {{}}\n\n{GONE}"));
    fixture.write("Cargo.lock", "# generated\n[[package]]\nname = \"a\"\n");
    let base = fixture.commit();
    fixture.write("src/lib.rs", "fn keep() {}\n");
    fixture.write("Cargo.lock", "# generated\n[[package]]\nname = \"b\"\n");
    let head = fixture.commit();
    let output = fixture.run(&base, &head);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = records(&output);

    let lock = record(&records, "Cargo.lock");
    assert_eq!(
        lock["visibility"],
        serde_json::json!({"collapsed": true, "label": "Generated file · hidden by default"})
    );
    let start = &records[0];
    assert!(start["files"]
        .as_array()
        .unwrap()
        .iter()
        .all(|file| file.get("visibility").is_none()));

    let lib = record(&records, "src/lib.rs");
    assert!(lib.get("visibility").is_none());
    let lhs = regions(lib, "lhs");
    let body = lhs
        .iter()
        .find(|region| {
            region["tags"]
                .as_array()
                .is_some_and(|tags| tags.iter().any(|tag| tag == "deleted-bodies:function"))
        })
        .expect("the deleted body");
    assert_eq!(body["visibility"]["label"], "12 lines removed");
    let docstring = lhs
        .iter()
        .find(|region| {
            region["tags"]
                .as_array()
                .is_some_and(|tags| tags.iter().any(|tag| tag == "deleted-bodies:docstring"))
        })
        .expect("the docstring");
    assert_eq!(docstring["fold_state_id"], body["fold_state_id"]);
    assert_eq!(docstring["visibility"]["collapsed"], true);
}

#[test]
fn a_failing_plugin_aborts_the_run() {
    let fixture = Fixture::new();
    fixture.config_file(
        "config.toml",
        "[plugins.bundled.summarize]\nenabled = true\napi_key = 'k'\nendpoint = 'http://127.0.0.1:1'\nretries = 0\nmin_lines = 1\n",
    );
    fixture.write("keep.txt", "keep\n");
    let base = fixture.commit();
    fixture.write("new.py", "def f():\n    a()\n    b()\n");
    fixture.remove("keep.txt");
    let head = fixture.commit();
    let output = fixture.run(&base, &head);
    assert_eq!(output.status.code(), Some(2));
    let records = records(&output);
    let aborted = &records.last().unwrap()["aborted"];
    assert_eq!(aborted["code"], "mutation_failed", "{aborted}");
    assert!(aborted["message"]
        .as_str()
        .unwrap()
        .starts_with("mutation summarize: summarizer: "));
}

#[test]
fn syntax_spans_come_only_with_the_flag() {
    let fixture = Fixture::new();
    fixture.write("a.rs", "fn a() {}\n");
    let base = fixture.commit();
    fixture.write("a.rs", "fn b() {}\n");
    let head = fixture.commit();
    let plain = records(&fixture.run(&base, &head));
    assert!(record(&plain, "a.rs")["diff"]["rhs"]
        .get("syntax")
        .is_none());
    let output = fixture.diffr(&[&base, &head, "--format", "ndjson", "--syntax"]);
    let records = records(&output);
    let syntax = record(&records, "a.rs")["diff"]["rhs"]["syntax"]
        .as_array()
        .unwrap()
        .clone();
    assert!(
        syntax
            .iter()
            .any(|span| span["capture"] == "keyword" && span["start_column"] == 0),
        "{syntax:?}"
    );
}

#[test]
fn git_binary_files_stream_as_diff_records_with_side_sizes() {
    let fixture = Fixture::new();
    fixture.write("keep.txt", "unchanged\n");
    let empty = fixture.commit();
    fixture.write("plugin.wasm", "\0asm\x01\0\0\0");
    let added = fixture.commit();
    fixture.write("plugin.wasm", "\0asm\x01\0\0\0extra");
    let modified = fixture.commit();
    fixture.remove("plugin.wasm");
    let deleted = fixture.commit();
    for (base, head, expected) in [
        (
            &empty,
            &added,
            serde_json::json!({"type": "binary", "rhs": {"size": 8}}),
        ),
        (
            &added,
            &modified,
            serde_json::json!({"type": "binary", "lhs": {"size": 8}, "rhs": {"size": 13}}),
        ),
        (
            &modified,
            &deleted,
            serde_json::json!({"type": "binary", "lhs": {"size": 13}}),
        ),
    ] {
        let output = fixture.run(base, head);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let events = records(&output);
        assert_eq!(events[1]["diff"], expected);
        assert!(events[1].get("error").is_none());
        assert_eq!(events.last().unwrap()["succeeded"], 1);
        assert_eq!(events.last().unwrap()["failed"], 0);
    }
    // The working-tree source uses the same binary path as a committed blob.
    fixture.write("plugin.wasm", "\0asm");
    let mut index = fixture.repo.index().unwrap();
    index.add_path(std::path::Path::new("plugin.wasm")).unwrap();
    index.write().unwrap();
    let events = records(&fixture.diffr(&[&added, "--format", "ndjson"]));
    assert_eq!(
        events[1]["diff"],
        serde_json::json!({"type": "binary", "lhs": {"size": 8}, "rhs": {"size": 4}})
    );
}

/// Stdout of a successful run.
fn stdout(output: &Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout.clone()).unwrap()
}

/// Twenty numbered lines of a function body.
fn body(name: &str) -> String {
    let lines: String = (1..=20).map(|i| format!("    let v{i} = {i};\n")).collect();
    format!("fn {name}() {{\n{lines}}}\n")
}

/// A comparison touching every kind of file change.
fn changes() -> (Fixture, String, String) {
    let fixture = Fixture::new();
    fixture.write(
        "src/lib.rs",
        &format!("{}\nfn f() {{ old(); }}\n", body("keep")),
    );
    fixture.write("src/moved.rs", &body("moved"));
    fixture.write("gone.rs", &body("gone"));
    fixture.write("tests/it.rs", "#[test]\nfn t() {}\n");
    fixture.write("logo.png", "\0PNG\x01\0");
    let base = fixture.commit();
    fixture.write(
        "src/lib.rs",
        &format!("{}\nfn f() {{ new(); }}\n", body("keep")),
    );
    fixture.remove("src/moved.rs");
    fixture.write("src/renamed.rs", &body("moved"));
    fixture.remove("gone.rs");
    fixture.write("src/new.rs", "fn added() {}\n");
    fixture.write("tests/it.rs", "#[test]\nfn t() { assert!(true); }\n");
    fixture.write("logo.png", "\0PNG\x02\0");
    let head = fixture.commit();
    (fixture, base, head)
}

#[test]
fn patch_output_has_git_headers_and_folds_what_starts_collapsed() {
    let (fixture, base, head) = changes();
    let text = stdout(&fixture.diffr(&[&base, &head, "--format", "patch"]));
    assert_eq!(
        text,
        "diff --git a/gone.rs b/gone.rs
deleted file mode 100644
@@ … file folded: Deleted file · hidden by default, +0 −22 @@
diff --git a/logo.png b/logo.png
Binary files a/logo.png and b/logo.png differ
diff --git a/src/lib.rs b/src/lib.rs
 1  1  fn keep() {
@@ … 19 unchanged lines · fn keep() @@
21 21      let v20 = 20;
22 22  }
23 23  
24    -fn f() { old(); }
   24 +fn f() { new(); }
diff --git a/src/new.rs b/src/new.rs
new file mode 100644
  1 +fn added() {}
diff --git a/src/moved.rs b/src/renamed.rs
rename from src/moved.rs
rename to src/renamed.rs
diff --git a/tests/it.rs b/tests/it.rs
@@ … file folded: Test file · hidden by default, +1 −1 @@
"
    );
    // Files finish in any order on the workers and print in file order.
    let serial = stdout(&fixture.diffr(&[&base, &head, "--format", "patch", "-j", "1"]));
    assert_eq!(serial, text);
}

#[test]
fn no_folds_prints_hidden_files_and_collapsed_regions_in_full() {
    let (fixture, base, head) = changes();
    let text = stdout(&fixture.diffr(&[
        &base,
        &head,
        "--format",
        "patch",
        "--no-folds",
        "--",
        "src/lib.rs",
        "tests/",
    ]));
    assert!(!text.contains("@@"), "{text}");
    assert!(text.contains("11 11      let v10 = 10;\n"), "{text}");
    assert!(
        text.ends_with(
            "diff --git a/tests/it.rs b/tests/it.rs
1 1  #[test]
2   -fn t() {}
  2 +fn t() { assert!(true); }
"
        ),
        "{text}"
    );
}

#[test]
fn patch_output_takes_unified_reverse_and_the_engine_limits() {
    let (fixture, base, head) = changes();
    let patch = |extra: &[&str]| {
        let mut args = vec![base.as_str(), head.as_str(), "--format", "patch"];
        args.extend_from_slice(extra);
        args.extend_from_slice(&["--", "src/lib.rs"]);
        stdout(&fixture.diffr(&args))
    };
    let text = patch(&["-U", "0"]);
    assert_eq!(
        text,
        "diff --git a/src/lib.rs b/src/lib.rs\n@@ … 23 unchanged lines @@\n24    -fn f() { old(); }\n   24 +fn f() { new(); }\n"
    );
    assert!(patch(&["-R"]).contains("24    -fn f() { new(); }\n   24 +fn f() { old(); }\n"));
    assert!(patch(&["--graph-limit", "1"]).contains(
        "diff --git a/src/lib.rs b/src/lib.rs\nLine diff (too_complex): structural diff exceeded diff.graph_limit (1); raise it in diffr config\n"
    ));
}

#[test]
fn patch_output_rejects_ndjson_only_flags() {
    let (fixture, base, head) = changes();
    for flag in ["--syntax", "--stream-annotations"] {
        let output = fixture.diffr(&[&base, &head, "--format", "patch", flag]);
        assert_eq!(output.status.code(), Some(2), "{flag}");
    }
    let output = fixture.diffr(&[&base, &head, "--format", "ndjson", "--no-folds"]);
    assert_eq!(output.status.code(), Some(2));
}
