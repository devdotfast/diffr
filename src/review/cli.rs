//! Git ref input for the experimental review snapshot command.
use crate::config::Config;
use crate::git::{DiffSession, FileParams, Result};
use clap::{Arg, Command as App};
use git2::Repository;
use std::path::{Component, Path};
use std::sync::Arc;

pub(crate) fn run() -> Result<()> {
    let args = App::new("difft review")
        .about("Experimental syntax-context diff snapshot; fold candidates stay expanded")
        .arg(
            Arg::new("format")
                .long("format")
                .value_parser(["text", "json", "viewer"])
                .default_value("text"),
        )
        .arg(Arg::new("config").long("config"))
        .arg(Arg::new("repo").long("repo").required(true))
        .arg(Arg::new("base").long("base").required(true))
        .arg(Arg::new("head").long("head").required(true))
        .arg(Arg::new("path").long("path"))
        .get_matches_from(
            std::iter::once(std::ffi::OsString::from("difft review"))
                .chain(std::env::args_os().skip(2)),
        );
    let repo = Repository::discover(args.get_one::<String>("repo").unwrap())?;
    let workspace = repo.workdir().unwrap_or(repo.path());
    let params =
        Config::load(workspace, args.get_one::<String>("config").map(Path::new))?.compile()?;
    let paths = args
        .get_one::<String>("path")
        .into_iter()
        .cloned()
        .collect::<Vec<_>>();
    if paths.iter().any(|path| {
        path.is_empty()
            || Path::new(path)
                .components()
                .any(|c| !matches!(c, Component::Normal(_)))
    }) {
        return Err(
            "Expected a repository-relative file path without '.' or '..' components".into(),
        );
    }
    let session = DiffSession::open(
        workspace,
        args.get_one::<String>("base").unwrap(),
        args.get_one::<String>("head").unwrap(),
        Arc::new(params),
        &FileParams {
            paths,
            ..FileParams::default()
        },
    )?;
    for (_, result) in session {
        let result = result?;
        match args.get_one::<String>("format").unwrap().as_str() {
            "json" => println!("{}", serde_json::to_string(&result.domain_json())?),
            "viewer" => println!("{}", serde_json::to_string(&result.viewer_json())?),
            _ => print!("{}", result.snapshot()),
        }
    }
    Ok(())
}
