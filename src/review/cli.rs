//! Git ref input for the experimental review snapshot command.
use crate::summary::DiffResult;
use clap::{Arg, Command as App};
use git2::{Commit, ErrorCode, Repository};
use std::path::{Component, Path};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn read_blob(repo: &Repository, commit: &Commit<'_>, path: &str) -> Result<Option<String>> {
    let tree = commit.tree()?;
    let entry = match tree.get_path(Path::new(path)) {
        Ok(entry) => entry,
        Err(error) if error.code() == ErrorCode::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if !matches!(entry.filemode(), 0o100644 | 0o100755) {
        return Err(
            "Review v0 requires a regular file path, not a directory, symlink or submodule".into(),
        );
    }
    let blob = repo.find_blob(entry.id())?;
    let bytes = blob.content();
    if bytes.contains(&0) {
        return Err("Review v0 supports text files only".into());
    }
    Ok(Some(std::str::from_utf8(bytes)?.to_owned()))
}

pub(crate) fn run() -> Result<()> {
    let args = App::new("difft review")
        .about("Experimental syntax-context diff snapshot; fold candidates stay expanded")
        .arg(
            Arg::new("format")
                .long("format")
                .value_parser(["text", "json", "viewer"])
                .default_value("text"),
        )
        .arg(Arg::new("repo").long("repo").required(true))
        .arg(Arg::new("base").long("base").required(true))
        .arg(Arg::new("head").long("head").required(true))
        .arg(Arg::new("path").long("path").required(true))
        .get_matches_from(
            std::iter::once(std::ffi::OsString::from("difft review"))
                .chain(std::env::args_os().skip(2)),
        );
    let repo = Repository::discover(args.get_one::<String>("repo").unwrap())?;
    let path = args.get_one::<String>("path").unwrap();
    if path.is_empty()
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(
            "Expected a repository-relative file path without '.' or '..' components".into(),
        );
    }
    let base = repo
        .revparse_single(args.get_one::<String>("base").unwrap())?
        .peel_to_commit()?;
    let head = repo
        .revparse_single(args.get_one::<String>("head").unwrap())?
        .peel_to_commit()?;
    let lhs = read_blob(&repo, &base, path)?;
    let rhs = read_blob(&repo, &head, path)?;
    if lhs.is_none() && rhs.is_none() {
        return Err(format!("{path} is absent at both refs").into());
    }
    // A missing side of an added/deleted file is represented by empty source.
    let result = DiffResult::from_sources(path, &lhs.unwrap_or_default(), &rhs.unwrap_or_default());
    match args.get_one::<String>("format").unwrap().as_str() {
        "json" => println!("{}", serde_json::to_string_pretty(&result.domain_json())?),
        "viewer" => println!("{}", serde_json::to_string(&result.viewer_json())?),
        _ => print!("{}", result.snapshot()),
    }
    Ok(())
}
