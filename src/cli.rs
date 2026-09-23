//! Git-style CLI input: the terminal UI, the NDJSON stream, and Git metadata.
use crate::config::{self, Config};
use crate::git::{Comparison, DiffSession, FileParams, Operand, Result};
use crate::options::DiffOptions;
use crate::plugin::Pipeline;
use clap::{Arg, ArgAction, ArgGroup, ArgMatches, Command};
use git2::{DiffStatsFormat, Repository};
use std::{
    ffi::OsString,
    io::{self, IsTerminal, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

fn flag(name: &'static str) -> Arg {
    Arg::new(name).long(name).action(ArgAction::SetTrue)
}

pub(crate) fn run() -> Result<i32> {
    let mut argv: Vec<OsString> = std::env::args_os().collect();
    let frontend_args = argv[1..].to_vec();
    // Preserve the distinction between revisions and paths explicitly following --.
    let has_separator = argv.iter().any(|arg| arg == "--");
    let explicit_paths = argv
        .iter()
        .position(|arg| arg == "--")
        .map(|at| {
            let paths = argv.split_off(at + 1);
            argv.pop();
            paths
        })
        .unwrap_or_default();
    let args = Command::new(env!("CARGO_BIN_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .about("Structural diffs with Git-style comparison inputs")
        .arg(Arg::new("repo").long("repo").default_value("."))
        .arg(Arg::new("config_file").long("config").value_name("PATH").help("Replace the global configuration file"))
        .arg(
            Arg::new("jobs")
                .long("jobs")
                .short('j')
                .value_parser(clap::value_parser!(usize))
                .default_value("16")
                .help("Concurrent file diffs for --format ndjson; results are emitted as each finishes"),
        )
        .arg(Arg::new("order").long("order").value_delimiter(',').action(ArgAction::Append).help("File tag priority: files carrying an earlier listed tag come first"))
        .arg(flag("cached").visible_alias("staged"))
        .arg(flag("merge-base"))
        .arg(flag("no-index"))
        .arg(flag("reverse").short('R'))
        .arg(flag("exit-code"))
        .arg(flag("quiet"))
        .arg(flag("name-only"))
        .arg(flag("name-status"))
        .arg(flag("stat"))
        .arg(flag("numstat"))
        .arg(flag("shortstat"))
        .group(ArgGroup::new("metadata").args(["name-only", "name-status", "stat", "numstat", "shortstat"]))
        .arg(flag("null").short('z'))
        .arg(flag("no-renames"))
        .arg(flag("find-renames").short('M').conflicts_with("no-renames"))
        .arg(Arg::new("unified").short('U').long("unified").value_parser(clap::value_parser!(u32)).help("Unchanged lines kept around each change; defaults to plugins.bundled.context.lines"))
        .arg(Arg::new("format").long("format").value_parser(["ndjson"]).help("Write the event stream to stdout instead of opening the terminal UI"))
        .arg(flag("stream-annotations").requires("format").help("Emit initial files followed by deferred annotations (NDJSON v4)"))
        .arg(flag("syntax").help("Include every token's tree-sitter capture name in --format ndjson output"))
        .arg(Arg::new("width").long("width").value_parser(clap::value_parser!(usize)).help("Columns for --stat; defaults to the terminal's width"))
        .arg(flag("ignore-comments"))
        .arg(Arg::new("byte-limit").long("byte-limit").value_parser(clap::value_parser!(usize)))
        .arg(Arg::new("graph-limit").long("graph-limit").value_parser(clap::value_parser!(usize)))
        .arg(Arg::new("parse-error-limit").long("parse-error-limit").value_parser(clap::value_parser!(usize)))
        .arg(Arg::new("items").num_args(0..).value_parser(clap::value_parser!(OsString)))
        .subcommand(
            Command::new("config")
                .about("Show, edit, or open the settings screen for diffr's configuration")
                .arg(Arg::new("query").help("Initial search in the settings screen"))
                .subcommand(Command::new("schema").about("Print the configuration's JSON Schema"))
                .subcommand(
                    Command::new("show")
                        .about("Print the resolved configuration")
                        .arg(flag("json"))
                        .arg(flag("reveal").help("Do not redact the API key")),
                )
                .subcommand(
                    Command::new("set")
                        .about("Write one key to the global configuration file")
                        .arg(Arg::new("key").required(true))
                        .arg(Arg::new("value").required(true)),
                ),
        )
        .after_help("Examples:\n  diffr\n  diffr --cached\n  diffr main...HEAD -- src/\n  diffr --no-index -- before.rs after.rs\n  diffr main HEAD --format ndjson\n\nUnsupported Git flags are rejected; this is not a complete git diff implementation.")
        .get_matches_from(argv);
    if let Some(("config", sub)) = args.subcommand() {
        return run_config(&args, sub);
    }
    let streaming = args.contains_id("format");
    let metadata_or_quiet = args.get_flag("quiet") || args.contains_id("metadata");
    if !streaming && !metadata_or_quiet {
        return launch_tui(&frontend_args, true);
    }
    if streaming && metadata_or_quiet {
        return Err("--format ndjson cannot be combined with --quiet or metadata output".into());
    }
    let stream_options = crate::protocol::stream::Options {
        syntax: args.get_flag("syntax"),
        updates: args.get_flag("stream-annotations"),
    };
    let items: Vec<OsString> = args
        .get_many::<OsString>("items")
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    if args.get_flag("no-index") {
        return no_index(
            &args,
            items.into_iter().chain(explicit_paths).collect(),
            stream_options,
        );
    }
    if args.get_flag("null") && !args.get_flag("name-only") && !args.get_flag("name-status") {
        return Err("-z currently requires --name-only or --name-status".into());
    }
    let location = std::fs::canonicalize(args.get_one::<String>("repo").unwrap())?;
    let repo = Repository::discover(&location)?;
    let workspace = repo.workdir().unwrap_or(repo.path());
    let (comparison, paths) = select(
        &repo,
        &location,
        &args,
        items,
        explicit_paths,
        has_separator,
    )?;
    let files = FileParams {
        paths,
        renames: !args.get_flag("no-renames"),
        order: args
            .get_many::<String>("order")
            .into_iter()
            .flatten()
            .cloned()
            .collect(),
    };
    if metadata_or_quiet {
        let diff = comparison.resolve(&repo)?.diff(&repo, &files)?;
        let changed = diff.deltas().len() > 0;
        if !args.get_flag("quiet") {
            let width = args
                .get_one::<usize>("width")
                .copied()
                .unwrap_or_else(crate::options::detect_terminal_width);
            print_metadata(&diff, &args, width)?;
        }
        return Ok(i32::from(
            changed && (args.get_flag("exit-code") || args.get_flag("quiet")),
        ));
    }
    let mut config = load_config(&args)?;
    apply_unified(&args, &mut config);
    let pipeline =
        Pipeline::from_config(&config.plugins, workspace).map_err(|error| format!("{error:#}"))?;
    let params = Arc::new(config.compile_with(&pipeline)?);
    let diff_options = diff_options(&args, &params);
    let mut session = DiffSession::open(
        workspace,
        comparison,
        Arc::clone(&params),
        &files,
        &pipeline,
    )?;
    session.diff_options = diff_options;
    let changed = session.remaining() > 0;
    let jobs = *args.get_one::<usize>("jobs").unwrap();
    if jobs == 0 {
        return Err("--jobs must be at least 1".into());
    }
    let ended = crate::protocol::stream::write(
        session,
        jobs,
        Arc::new(pipeline),
        stream_options,
        &mut io::stdout().lock(),
    )?;
    Ok(if ended.failed || ended.aborted {
        2
    } else {
        i32::from(changed && args.get_flag("exit-code"))
    })
}

fn select(
    repo: &Repository,
    location: &Path,
    args: &ArgMatches,
    items: Vec<OsString>,
    explicit_paths: Vec<OsString>,
    has_separator: bool,
) -> Result<(Comparison, Vec<String>)> {
    let mut revisions = Vec::new();
    let mut paths = Vec::new();
    for item in items {
        let text = item
            .to_str()
            .ok_or("non-UTF-8 revision/path arguments are unsupported")?;
        let is_rev = repo.revparse(text).is_ok();
        let is_path = location.join(&item).exists();
        if is_rev && is_path && !has_separator {
            return Err(
                format!("ambiguous revision and path {text:?}; use -- to separate them").into(),
            );
        }
        if is_rev && paths.is_empty() {
            revisions.push(text.to_owned());
        } else if is_rev {
            return Err("revisions must precede paths; use -- to separate them".into());
        } else if is_path {
            paths.push(item);
        } else {
            return Err(
                format!("unknown revision or path {text:?}; use -- before pathspecs").into(),
            );
        }
    }
    paths.extend(explicit_paths);
    // Match canonical path forms, including Windows verbatim path prefixes.
    let root = std::fs::canonicalize(repo.workdir().unwrap_or(repo.path()))?;
    let prefix = location.strip_prefix(&root)?;
    let paths = paths
        .into_iter()
        .map(|path| normalize_path(prefix, &path))
        .collect::<Result<Vec<_>>>()?;
    let cached = args.get_flag("cached");
    let mut comparison = match revisions.as_slice() {
        [] if cached => Comparison {
            before: match repo.head() {
                Ok(_) => Operand::revision("HEAD"),
                Err(error) if error.code() == git2::ErrorCode::UnbornBranch => Operand::EmptyTree,
                Err(error) => return Err(error.into()),
            },
            after: Operand::Index,
        },
        [] => Comparison {
            before: Operand::Index,
            after: Operand::WorkingTree,
        },
        [range] if range.contains("..") => {
            if cached {
                return Err("--cached takes one revision, not a range".into());
            }
            let (a, b, merge) = if let Some((a, b)) = range.split_once("...") {
                (a, b, true)
            } else {
                let (a, b) = range.split_once("..").unwrap();
                (a, b, false)
            };
            let a = if a.is_empty() { "HEAD" } else { a };
            let b = if b.is_empty() { "HEAD" } else { b };
            Comparison {
                before: if merge {
                    merge_base(repo, a, b)?
                } else {
                    Operand::revision(a)
                },
                after: Operand::revision(b),
            }
        }
        [rev] => Comparison {
            before: Operand::revision(rev),
            after: if cached {
                Operand::Index
            } else {
                Operand::WorkingTree
            },
        },
        [a, b] if !cached => Comparison {
            before: Operand::revision(a),
            after: Operand::revision(b),
        },
        _ => return Err("expected at most two revisions (--cached takes at most one)".into()),
    };
    if args.get_flag("merge-base") {
        let a = revisions
            .first()
            .ok_or("--merge-base requires a revision")?;
        if a.contains("..") {
            return Err("do not combine --merge-base with a range".into());
        }
        comparison.before = merge_base(
            repo,
            a,
            revisions.get(1).map(String::as_str).unwrap_or("HEAD"),
        )?;
    }
    if args.get_flag("reverse") {
        comparison.reverse();
    }
    Ok((comparison, paths))
}

fn merge_base(repo: &Repository, a: &str, b: &str) -> Result<Operand> {
    let a = repo.revparse_single(a)?.peel_to_commit()?.id();
    let b = repo.revparse_single(b)?.peel_to_commit()?.id();
    Ok(Operand::revision(repo.merge_base(a, b)?.to_string()))
}

fn normalize_path(prefix: &Path, path: &std::ffi::OsStr) -> Result<String> {
    if path.to_string_lossy().starts_with(':') {
        return Err("Git magic pathspecs are not supported".into());
    }
    let mut normalized = PathBuf::new();
    for part in prefix.join(path).components() {
        match part {
            Component::Normal(part) => normalized.push(part),
            Component::CurDir => {}
            Component::ParentDir if normalized.pop() => {}
            _ => return Err("pathspec must stay inside the repository".into()),
        }
    }
    Ok(normalized.to_str().ok_or("non-UTF-8 pathspec")?.to_owned())
}

fn print_metadata(diff: &git2::Diff<'_>, args: &ArgMatches, width: usize) -> Result<()> {
    let mut stdout = io::stdout().lock();
    if args.get_flag("name-only") || args.get_flag("name-status") {
        let separator: &[u8] = if args.get_flag("null") { b"\0" } else { b"\t" };
        let terminator: &[u8] = if args.get_flag("null") { b"\0" } else { b"\n" };
        for delta in diff.deltas() {
            if args.get_flag("name-status") {
                write!(
                    stdout,
                    "{}",
                    match delta.status() {
                        git2::Delta::Added => 'A',
                        git2::Delta::Deleted => 'D',
                        git2::Delta::Renamed => 'R',
                        git2::Delta::Typechange => 'T',
                        git2::Delta::Conflicted => 'U',
                        _ => 'M',
                    }
                )?;
                stdout.write_all(separator)?;
                if delta.status() == git2::Delta::Renamed {
                    stdout.write_all(delta.old_file().path_bytes().unwrap())?;
                    stdout.write_all(separator)?;
                }
            }
            stdout.write_all(
                delta
                    .new_file()
                    .path_bytes()
                    .or(delta.old_file().path_bytes())
                    .ok_or("missing path")?,
            )?;
            stdout.write_all(terminator)?;
        }
        return Ok(());
    }
    if args.get_flag("numstat") {
        for (index, delta) in diff.deltas().enumerate() {
            let patch = git2::Patch::from_diff(diff, index)?;
            if let Some(patch) = patch {
                let (_, additions, deletions) = patch.line_stats()?;
                write!(stdout, "{additions}\t{deletions}\t")?;
            } else {
                write!(stdout, "-\t-\t")?;
            }
            if delta.status() == git2::Delta::Renamed {
                stdout.write_all(delta.old_file().path_bytes().ok_or("missing old path")?)?;
                stdout.write_all(b" => ")?;
            }
            stdout.write_all(
                delta
                    .new_file()
                    .path_bytes()
                    .or(delta.old_file().path_bytes())
                    .ok_or("missing path")?,
            )?;
            stdout.write_all(b"\n")?;
        }
        return Ok(());
    }
    let format = if args.get_flag("shortstat") {
        DiffStatsFormat::SHORT
    } else {
        DiffStatsFormat::FULL
    };
    stdout.write_all(&diff.stats()?.to_buf(format, width)?)?;
    Ok(())
}

fn no_index(
    args: &ArgMatches,
    paths: Vec<OsString>,
    stream_options: crate::protocol::stream::Options,
) -> Result<i32> {
    if paths.len() != 2 {
        return Err("--no-index requires two file paths".into());
    }
    if args.get_flag("cached")
        || args.get_flag("merge-base")
        || args.contains_id("metadata")
        || args.get_flag("null")
    {
        return Err("--no-index currently supports structural file output and --quiet, not index or metadata options".into());
    }
    let mut paths = paths;
    if args.get_flag("reverse") {
        paths.swap(0, 1);
    }
    let read = |path: &OsString| -> Result<Vec<u8>> {
        if path == "/dev/null" {
            return Ok(Vec::new());
        }
        Ok(std::fs::read(path)?)
    };
    let before = read(&paths[0])?;
    let after = read(&paths[1])?;
    let changed = before != after;
    if args.get_flag("quiet") {
        return Ok(i32::from(changed));
    }
    let mut config = load_config(args)?;
    apply_unified(args, &mut config);
    let pipeline = Pipeline::from_config(&config.plugins, &std::env::current_dir()?)
        .map_err(|error| format!("{error:#}"))?;
    let config = config.compile_with(&pipeline)?;
    let options = &diff_options(args, &config);
    let lhs = crate::options::FileArgument::from_path_argument(&paths[0]);
    let rhs = crate::options::FileArgument::from_path_argument(&paths[1]);
    let compute = || {
        crate::diff_file(
            &config,
            &paths[1].to_string_lossy(),
            &lhs,
            &rhs,
            options,
            false,
            &[],
            &[],
        )
    };
    let ended = crate::protocol::stream::write_file(
        &paths[0].to_string_lossy(),
        &paths[1].to_string_lossy(),
        (before.len() as u64, after.len() as u64),
        compute,
        &config,
        &pipeline,
        stream_options,
        &mut io::stdout().lock(),
    )?;
    Ok(if ended.failed || ended.aborted {
        2
    } else {
        i32::from(changed && args.get_flag("exit-code"))
    })
}

/// The engine limits: the configured `[diff]` table, then the command-line
/// flags.
fn diff_options(args: &ArgMatches, params: &config::Params) -> DiffOptions {
    let mut options = params.diff.options(args.get_flag("ignore-comments"));
    if let Some(limit) = args.get_one::<usize>("byte-limit") {
        options.byte_limit = *limit;
    }
    if let Some(limit) = args.get_one::<usize>("graph-limit") {
        options.graph_limit = *limit;
    }
    if let Some(limit) = args.get_one::<usize>("parse-error-limit") {
        options.parse_error_limit = *limit;
    }
    options
}

/// `-U` overrides the context plugin's `lines` for this run.
fn apply_unified(args: &ArgMatches, config: &mut Config) {
    let Some(unified) = args.get_one::<u32>("unified") else {
        return;
    };
    if let Some(entry) = config.plugins.entries.get_mut("bundled.context") {
        entry
            .options
            .insert("lines".into(), serde_json::Value::from(*unified));
    }
}

/// The global file, or the `--config` file in its place.
fn load_config(args: &ArgMatches) -> Result<Config> {
    Ok(Config::load(
        args.get_one::<String>("config_file").map(Path::new),
    )?)
}

/// `diffr config`: settings, schema, resolved values and edits.
fn run_config(args: &ArgMatches, sub: &ArgMatches) -> Result<i32> {
    let mut stdout = io::stdout().lock();
    match sub.subcommand() {
        Some(("schema", _)) => {
            serde_json::to_writer_pretty(&mut stdout, &Config::schema())?;
            stdout.write_all(b"\n")?;
        }
        Some(("show", show)) => {
            let config = load_config(args)?;
            let reveal = show.get_flag("reveal");
            if show.get_flag("json") {
                serde_json::to_writer_pretty(&mut stdout, &config::store::show(&config, reveal))?;
                stdout.write_all(b"\n")?;
            } else {
                stdout.write_all(
                    toml::to_string_pretty(&config::store::redacted(&config, reveal))?.as_bytes(),
                )?;
            }
        }
        Some(("set", set)) => {
            let path = match args.get_one::<String>("config_file") {
                Some(path) => PathBuf::from(path),
                None => config::global_path()?,
            };
            config::store::set(
                &path,
                set.get_one::<String>("key").unwrap(),
                set.get_one::<String>("value").unwrap(),
            )?;
        }
        Some((other, _)) => return Err(format!("unknown config command {other}").into()),
        None => {
            let mut frontend = vec![OsString::from("--settings")];
            if let Some(query) = sub.get_one::<String>("query") {
                frontend.push(query.into());
            }
            return launch_tui(&frontend, false);
        }
    }
    Ok(0)
}

/// `comparison` passes the arguments after `--` as the comparison to open;
/// otherwise they are frontend flags such as `--settings`.
fn launch_tui(args: &[OsString], comparison: bool) -> Result<i32> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        let alternative = if comparison {
            "--format ndjson"
        } else {
            "config show/set"
        };
        return Err(format!(
            "diffr's terminal UI needs a terminal; use {alternative} for non-interactive use"
        )
        .into());
    }
    let mut command = if let Some(entry) = std::env::var_os("DIFFR_TUI_ENTRY") {
        let bun = std::env::var_os("DIFFR_BUN").unwrap_or_else(|| "bun".into());
        let mut command = std::process::Command::new(bun);
        command.arg("run").arg(entry);
        command
    } else {
        let sibling = std::env::current_exe()?
            .with_file_name(format!("diffr-tui{}", std::env::consts::EXE_SUFFIX));
        std::process::Command::new(if sibling.is_file() {
            sibling
        } else {
            PathBuf::from("diffr-tui")
        })
    };
    if comparison {
        command
            .arg("--diffr")
            .arg(std::env::current_exe()?)
            .arg("--")
            .args(args);
    } else {
        // `bun run main.tsx --settings --diffr <exe> [query]`
        command
            .arg(&args[0])
            .arg("--diffr")
            .arg(std::env::current_exe()?)
            .args(&args[1..]);
    }
    // Unix exec replaces this process, so signals to diffr's PID reach the TUI
    // directly. std has no portable exec; other platforms spawn and wait.
    #[cfg(unix)]
    let result: io::Result<i32> = {
        use std::os::unix::process::CommandExt;
        Err(command.exec())
    };
    #[cfg(not(unix))]
    let result = command.status().map(|status| status.code().unwrap_or(2));

    result.map_err(|error| {
        format!("Could not launch terminal frontend: {error}. Run cargo xtask install-tui from the checkout to install the frontend, or use --format ndjson. For source development, set DIFFR_TUI_ENTRY and ensure Bun is available.").into()
    })
}
