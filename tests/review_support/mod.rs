//! Fixture loading and assertions. Test cases live in ../review.rs.
mod git;
pub(crate) use git::Repo;

use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
struct Provenance {
    sources: Sources,
}

#[derive(Deserialize)]
struct Sources {
    lhs: Source,
    rhs: Source,
}

#[derive(Deserialize)]
struct Source {
    file: String,
    path: Option<String>,
    git_blob_sha: Option<String>,
}

pub(crate) struct Fixture {
    name: String,
    directory: PathBuf,
    repo: Repo,
    base: String,
    head: String,
    path: String,
    sources: [String; 2],
}

impl Fixture {
    /// Restore the pinned source blobs as two local commits. No network needed.
    pub(crate) fn load(name: &str) -> Self {
        let directory = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples/review/real")
            .join(name);
        let provenance: Provenance =
            serde_json::from_slice(&fs::read(directory.join("provenance.json")).unwrap()).unwrap();
        let Sources { lhs, rhs } = provenance.sources;
        let path = match (&lhs.path, &rhs.path) {
            (Some(left), Some(right)) => {
                assert_eq!(left, right, "Rename fixtures need separate base/head paths");
                right.clone()
            }
            (Some(path), None) | (None, Some(path)) => path.clone(),
            (None, None) => panic!("Fixture is missing both source paths"),
        };
        let before = fs::read_to_string(directory.join(&lhs.file)).unwrap();
        let after = fs::read_to_string(directory.join(&rhs.file)).unwrap();
        let repo = Repo::new();
        let base = restore_commit(&repo, &path, &lhs, &before, None);
        let head = restore_commit(&repo, &path, &rhs, &after, Some(&base));
        Self {
            name: name.into(),
            directory,
            repo,
            base,
            head,
            path,
            sources: [before, after],
        }
    }

    /// Invoke the actual CLI using the repository, base/head refs and file path.
    pub(crate) fn run(&self) -> String {
        let output = self.repo.review(&self.base, &self.head, &self.path);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// Check source/context requirements, then compare the complete golden.
    /// Updating a golden cannot bypass the required return/brace checks.
    pub(crate) fn assert_expected(&self, actual: &str) {
        self.assert_source_and_context(actual);
        let golden = self.directory.join("review.snap");
        if std::env::var_os("UPDATE_REVIEW_GOLDENS").as_deref() == Some(std::ffi::OsStr::new("1")) {
            fs::write(&golden, actual).unwrap();
        }
        let expected = fs::read_to_string(&golden)
            .expect("Missing review.snap: run UPDATE_REVIEW_GOLDENS=1 cargo test --test review");
        pretty_assertions::assert_eq!(expected, actual, "{}", self.name);
    }

    fn assert_source_and_context(&self, actual: &str) {
        let mut shown: [std::collections::BTreeSet<usize>; 2] = Default::default();
        for line in actual.lines() {
            if line.len() < 11 {
                continue;
            }
            let bytes = line.as_bytes();
            if !bytes[..10].iter().all(|c| c.is_ascii_digit() || *c == b' ') {
                continue;
            }
            for (side, number) in [line[..4].trim(), line[5..9].trim()].iter().enumerate() {
                if let Ok(number) = number.parse::<usize>() {
                    let index = number - 1;
                    assert!(shown[side].insert(index), "Duplicate source row: {line}");
                    assert_eq!(
                        &line[12..],
                        self.sources[side].split('\n').nth(index).unwrap(),
                        "Wrong source text: {line}"
                    );
                }
            }
        }
        let case: Value =
            serde_json::from_slice(&fs::read(self.directory.join("case.json")).unwrap()).unwrap();
        for (side, key) in ["lhs", "rhs"].iter().enumerate() {
            if let Some(entries) = case["keep_visible"][key].as_array() {
                for entry in entries {
                    let start = entry["range"]["start"]["line"].as_u64().unwrap() as usize;
                    let end = entry["range"]["end"]["line"].as_u64().unwrap() as usize;
                    let exclusive = end
                        + usize::from(entry["range"]["end"]["byte_column"].as_u64().unwrap() > 0);
                    for row in start..exclusive {
                        assert!(
                            shown[side].contains(&row),
                            "{}: missing {key} line {}: {}",
                            self.name,
                            row + 1,
                            self.sources[side].split('\n').nth(row).unwrap()
                        );
                    }
                }
            }
        }
    }
}

fn restore_commit(
    repo: &Repo,
    path: &str,
    spec: &Source,
    text: &str,
    parent: Option<&str>,
) -> String {
    let contents = match &spec.path {
        Some(_) => Some(text.as_bytes()),
        None => {
            assert!(
                text.is_empty(),
                "An absent file must have empty fixture content"
            );
            None
        }
    };
    let (commit, blob) = repo.commit(path, contents, parent);
    assert_eq!(
        blob, spec.git_blob_sha,
        "Pinned source blob changed: {}",
        spec.file
    );
    commit
}
