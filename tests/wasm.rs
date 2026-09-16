//! WASM component plugins: the bundled plugins built as components shape
//! files exactly as the same source compiled into diffr does, and the
//! fixtures example classifies, reads a file, runs git and moves regions.
mod support;

use git2::{IndexAddOption, Repository, Signature, Time};
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Once;
use support::get_base_command;
use tempfile::TempDir;

/// Build every guest plugin into its folder's `plugin.wasm`, once per test
/// run.
fn build_plugins() {
    static BUILD: Once = Once::new();
    BUILD.call_once(|| {
        let output = Command::new("sh")
            .arg(root().join("scripts/build-wasm-plugins.sh"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "building the WASM plugins failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    });
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// A repository with a working tree, a home directory, and a config
/// directory.
struct Fixture {
    dir: TempDir,
    repo: Repository,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path().join("repo")).unwrap();
        fs::create_dir_all(dir.path().join("home/.config")).unwrap();
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

    fn commit(&self, message: &str) -> String {
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
                message,
                &tree,
                &parents,
            )
            .unwrap()
            .to_string()
    }

    /// A config file holding `text`, in a directory of its own.
    fn config(&self, name: &str, text: &str) -> PathBuf {
        let path = self.dir.path().join(name).join("config.toml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }

    fn run(&self, config: &Path, base: &str, head: &str) -> Output {
        let home = self.dir.path().join("home");
        get_base_command()
            .arg("--repo")
            .arg(self.dir.path().join("repo"))
            .arg("--config")
            .arg(config)
            .args([base, head, "--format", "ndjson"])
            .env("HOME", &home)
            .env("XDG_CONFIG_HOME", home.join(".config"))
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

fn path_of(file: &Value) -> &str {
    file.get("rhs").or_else(|| file.get("lhs")).unwrap()["path"]
        .as_str()
        .unwrap()
}

/// The start record, the file records sorted by path (they arrive in
/// completion order), and the complete record.
fn sorted(output: &Output) -> (Value, Vec<Value>, Value) {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut records = records(output);
    let complete = records.pop().unwrap();
    let start = records.remove(0);
    records.sort_by(|a, b| path_of(&a["file"]).cmp(path_of(&b["file"])));
    (start, records, complete)
}

fn walk<'a>(regions: &'a Value, out: &mut Vec<&'a Value>) {
    for region in regions.as_array().into_iter().flatten() {
        out.push(region);
        walk(&region["children"], out);
    }
}

fn file_record<'a>(records: &'a [Value], path: &str) -> &'a Value {
    records
        .iter()
        .find(|record| record["type"] == "file" && path_of(&record["file"]) == path)
        .unwrap_or_else(|| panic!("{path} has no file record"))
}

#[test]
fn bundled_plugins_as_components_shape_files_exactly_as_natively() {
    build_plugins();
    let fixture = Fixture::new();
    let body = |name: &str| {
        format!("def {name}():\n    a = 1\n    b = 2\n    c = 3\n    return a + b + c\n\n")
    };
    let rust_test = |name: &str| {
        format!("    /// Checks {name}.\n    /// Really.\n    #[test]\n    fn {name}() {{\n        let a = 1;\n        let b = 2;\n        assert_eq!(a + b, 3);\n    }}\n\n")
    };
    fixture.write(
        "src/removed.py",
        &format!("{}{}{}keep = 1\n", body("a"), body("b"), body("c")),
    );
    fixture.write(
        "src/lib.rs",
        &format!(
            "/// Adds.\n/// Twice.\npub fn add(a: u32) -> u32 {{\n    let b = a;\n    let c = b;\n    c + c\n}}\n\npub fn keep() -> u32 {{\n    1\n}}\n\n#[cfg(test)]\nmod tests {{\n{}{}}}\n",
            rust_test("one"),
            rust_test("two")
        ),
    );
    fixture.write(
        "tests/test_things.py",
        &format!(
            "def test_one():\n    assert 1\n    assert 2\n    assert 3\n\n\ndef test_two():\n    assert 1\n    assert 2\n    assert 3\n\n{}",
            body("helper")
        ),
    );
    fixture.write(
        "web/a.test.ts",
        "it('adds', () => {\n  expect(1).toBe(1);\n  expect(2).toBe(2);\n});\n\nit('subtracts', () => {\n  expect(1).toBe(1);\n  expect(2).toBe(2);\n});\n",
    );
    fixture.write("src/gone.go", "package a\n\n// Gone.\n// Really.\nfunc Gone() int {\n\ta := 1\n\tb := 2\n\treturn a + b\n}\n");
    let base = fixture.commit("base\n");
    fixture.write("src/removed.py", "keep = 1\n");
    fixture.write(
        "src/lib.rs",
        &format!(
            "pub fn keep() -> u32 {{\n    2\n}}\n\n#[cfg(test)]\nmod tests {{\n{}{}}}\n",
            rust_test("one").replace("a + b", "b + a"),
            rust_test("three")
        ),
    );
    fixture.write(
        "tests/test_things.py",
        "def test_one():\n    assert 1\n    assert 2\n    assert 4\n\n\ndef test_three():\n    assert 1\n    assert 2\n    assert 3\n",
    );
    fixture.write(
        "web/a.test.ts",
        "it('adds', () => {\n  expect(1).toBe(1);\n  expect(3).toBe(3);\n});\n",
    );
    fixture.remove("src/gone.go");
    fixture.write(
        "src/new.rs",
        "#[test]\nfn new() {\n    a();\n    b();\n    c();\n}\n",
    );
    let head = fixture.commit("head\n");

    let options = "[plugins.deleted-bodies]\nmin_lines = 3\n[plugins.test-bodies]\nmin_lines = 2\n";
    let native = fixture.config("native", options);
    let plugin = |name: &str| root().join("plugins").join(name).display().to_string();
    // Every bundled plugin that builds as a component; the summarizer is off.
    let wasm = fixture.config(
        "wasm",
        &format!(
            "[plugins.context]\npath = {:?}\n[plugins.hide-files]\npath = {:?}\n[plugins.deleted-bodies]\npath = {:?}\nmin_lines = 3\n[plugins.test-bodies]\npath = {:?}\nmin_lines = 2\n[plugins.removed-runs]\npath = {:?}\n[plugins.group]\npath = {:?}\n",
            plugin("context"),
            plugin("hide-files"),
            plugin("deleted-bodies"),
            plugin("test-bodies"),
            plugin("removed-runs"),
            plugin("group")
        ),
    );

    let native = sorted(&fixture.run(&native, &base, &head));
    let wasm = sorted(&fixture.run(&wasm, &base, &head));
    assert_eq!(native.1.len(), 6);
    // The runs shape something, so that equal streams mean something.
    let mut collapsed = Vec::new();
    for record in &native.1 {
        for side in ["lhs", "rhs"] {
            let mut regions = Vec::new();
            walk(&record["diff"][side]["regions"], &mut regions);
            collapsed.extend(
                regions
                    .into_iter()
                    .filter(|region| region["visibility"]["collapsed"] == true)
                    .filter_map(|region| region["visibility"]["label"].as_str()),
            );
        }
    }
    for label in [
        "2 collapsed regions · 7 lines",
        "test body",
        "test module",
        "4 lines removed",
    ] {
        assert!(collapsed.contains(&label), "{label}: {collapsed:?}");
    }
    assert_eq!(native, wasm);
}

/// The example's config: every bundled plugin, then `fixtures`, whose folder
/// is written relative to the config file.
fn fixtures_config(fixture: &Fixture, extra: &str) -> PathBuf {
    let config = fixture.config("fixtures", "");
    let relative = pathdiff(
        &root().join("examples/plugins/fixtures"),
        config.parent().unwrap(),
    );
    fs::write(
        &config,
        format!(
            "[plugins]\norder = [\"context\", \"hide-files\", \"deleted-bodies\", \"test-bodies\", \"removed-runs\", \"summarize\", \"group\", \"fixtures\"]\n[plugins.fixtures]\npath = {:?}\n{extra}",
            relative.display().to_string()
        ),
    )
    .unwrap();
    config
}

/// `path` relative to `base`, both absolute.
fn pathdiff(path: &Path, base: &Path) -> PathBuf {
    let path = fs::canonicalize(path).unwrap();
    let base = fs::canonicalize(base).unwrap();
    let common = path
        .components()
        .zip(base.components())
        .take_while(|(a, b)| a == b)
        .count();
    let mut relative = PathBuf::new();
    for _ in base.components().skip(common) {
        relative.push("..");
    }
    for component in path.components().skip(common) {
        relative.push(component);
    }
    relative
}

fn fixtures_repo() -> (Fixture, String, String) {
    let fixture = Fixture::new();
    let data = |last: &str| format!("header\none\ntwo\nthree\nfour\n{last}\n");
    fixture.write("fixtures/data.txt", &data("five"));
    fixture.write("src/marked.txt", &format!("// fixture\n{}", data("five")));
    fixture.write("src/plain.txt", &data("five"));
    // Deleted at head: the working tree no longer has it, so only the blob
    // the file entry names can say whether it was a fixture.
    fixture.write("src/gone.txt", &format!("// fixture\n{}", data("five")));
    let base = fixture.commit("Add fixtures\n");
    fixture.write("fixtures/data.txt", &data("six"));
    fixture.write("src/marked.txt", &format!("// fixture\n{}", data("six")));
    fixture.write("src/plain.txt", &data("six"));
    fixture.remove("src/gone.txt");
    let head = fixture.commit("Update fixtures\n");
    (fixture, base, head)
}

#[test]
fn the_fixtures_example_classifies_reads_files_runs_git_and_moves_regions() {
    build_plugins();
    let (fixture, base, head) = fixtures_repo();
    let output = fixture.run(&fixtures_config(&fixture, ""), &base, &head);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let records = records(&output);
    let tags = |path: &str| {
        records[0]["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| path_of(&entry["file"]) == path)
            .unwrap()
            .get("tags")
            .cloned()
    };
    assert_eq!(
        tags("fixtures/data.txt"),
        Some(serde_json::json!(["fixture"]))
    );
    assert_eq!(
        tags("src/marked.txt"),
        Some(serde_json::json!(["fixture"])),
        "the plugin reads the file and finds the marker"
    );
    assert_eq!(tags("src/plain.txt"), None);
    assert_eq!(
        tags("src/gone.txt"),
        Some(serde_json::json!(["fixture"])),
        "a deleted file is read from its blob, not the working tree"
    );

    for path in ["fixtures/data.txt", "src/marked.txt"] {
        let record = file_record(&records, path);
        assert_eq!(
            record["visibility"],
            serde_json::json!({"collapsed": true, "label": "Fixture · Update fixtures"}),
            "{path}"
        );
        let mut regions = Vec::new();
        walk(&record["diff"]["lhs"]["regions"], &mut regions);
        let pieces: Vec<&Value> = regions
            .into_iter()
            .filter(|region| {
                region["visibility"]["label"]
                    .as_str()
                    .is_some_and(|label| label.ends_with(" fixture lines"))
            })
            .collect();
        assert_eq!(pieces.len(), 1, "{path}: {record}");
        assert_eq!(pieces[0]["visibility"]["collapsed"], true);
    }
    let plain = file_record(&records, "src/plain.txt");
    assert!(plain.get("visibility").is_none(), "{plain}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("[fixtures] fixtures/data.txt: last changed in \"Update fixtures\""),
        "{stderr}"
    );
}

#[test]
fn a_guest_error_aborts_the_run_with_mutation_failed() {
    build_plugins();
    let (fixture, base, head) = fixtures_repo();
    let output = fixture.run(&fixtures_config(&fixture, "fail = true\n"), &base, &head);
    assert_eq!(output.status.code(), Some(2));
    let records = records(&output);
    let aborted = &records.last().unwrap()["aborted"];
    assert_eq!(aborted["code"], "mutation_failed", "{aborted}");
    let message = aborted["message"].as_str().unwrap();
    assert!(
        message.starts_with("mutation fixtures: asked to fail on "),
        "{message}"
    );
}
