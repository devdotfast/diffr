//! The plugins on the stream: hidden files, collapsed and linked regions, and
//! a run cut short by a failing plugin.
use git2::{IndexAddOption, Repository, Signature, Time};
use serde_json::Value;
use std::fs;
use std::process::{Command, Output};
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
        Command::new(assert_cmd::cargo_bin!("diffr"))
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
