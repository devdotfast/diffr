//! Temporary Git repositories used by the integration fixtures.
use git2::{Oid, Repository, Signature, Time};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

pub(crate) struct Repo(PathBuf);
impl Repo {
    pub(crate) fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "difft-review-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir(&path).unwrap();
        let repo = Self(path);
        Repository::init(&repo.0).unwrap();
        repo
    }
    pub(crate) fn commit(
        &self,
        path: &str,
        source: Option<&[u8]>,
        parent: Option<&str>,
    ) -> (String, Option<String>) {
        let repo = Repository::open(&self.0).unwrap();
        let blob = source.map(|source| repo.blob(source).unwrap());
        let tree_id = match blob {
            Some(blob) => file_tree(&repo, path, blob),
            None => repo.treebuilder(None).unwrap().write().unwrap(),
        };
        let tree = repo.find_tree(tree_id).unwrap();
        let parent = parent.map(|id| repo.find_commit(Oid::from_str(id).unwrap()).unwrap());
        let parents: Vec<_> = parent.iter().collect();
        let signature = Signature::new(
            "Fixture",
            "fixture@example.invalid",
            &Time::new(946684800, 0),
        )
        .unwrap();
        let commit = repo
            .commit(
                None,
                &signature,
                &signature,
                "Pinned fixture source\n",
                &tree,
                &parents,
            )
            .unwrap();
        (commit.to_string(), blob.map(|id| id.to_string()))
    }
    pub(crate) fn review(&self, base: &str, head: &str, path: &str) -> std::process::Output {
        Command::new(assert_cmd::cargo_bin!("difft"))
            .args(["--repo"])
            .arg(&self.0)
            .args([base, head, "--format", "snapshot", "--", path])
            .env_remove("DFT_DBG_KEEP_UNCHANGED")
            .output()
            .unwrap()
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

// Build only this fixture's path, without an index or worktree checkout.
fn file_tree(repo: &Repository, path: &str, blob: Oid) -> Oid {
    let mut tree = repo.treebuilder(None).unwrap();
    match path.split_once('/') {
        Some((directory, remaining)) => {
            let child = file_tree(repo, remaining, blob);
            tree.insert(directory, child, 0o040000).unwrap();
        }
        None => {
            tree.insert(path, blob, 0o100644).unwrap();
        }
    }
    tree.write().unwrap()
}
