//! Git-style CLI input; rendering and NDJSON remain adapters over the same engine.
use crate::config::Config;
use crate::git::{Comparison, DiffSession, FileParams, Operand, Result};
use crate::options::{DiffOptions, DisplayMode, DisplayOptions};
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
        .arg(Arg::new("config").long("config"))
        .arg(Arg::new("order").long("order").value_delimiter(',').action(ArgAction::Append).help("File class priority from diffr-classify attributes"))
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
        .arg(Arg::new("unified").short('U').long("unified").default_value("3").value_parser(clap::value_parser!(u32)))
        .arg(Arg::new("format").long("format").value_parser(["text", "json", "ndjson", "snapshot"]).default_value("text"))
        .arg(Arg::new("display").long("display").value_parser(["inline", "side-by-side", "side-by-side-show-both"]).default_value("side-by-side"))
        .arg(Arg::new("color").long("color").num_args(0..=1).require_equals(true).default_missing_value("always").default_value("auto").value_parser(["auto", "always", "never"]))
        .arg(flag("no-color"))
        .arg(Arg::new("width").long("width").value_parser(clap::value_parser!(usize)))
        .arg(flag("ignore-comments"))
        .arg(Arg::new("byte-limit").long("byte-limit").value_parser(clap::value_parser!(usize)))
        .arg(Arg::new("graph-limit").long("graph-limit").value_parser(clap::value_parser!(usize)))
        .arg(Arg::new("parse-error-limit").long("parse-error-limit").value_parser(clap::value_parser!(usize)))
        .arg(Arg::new("items").num_args(0..).value_parser(clap::value_parser!(OsString)))
        .after_help("Examples:\n  diffr\n  diffr --cached\n  diffr main...HEAD -- src/\n  diffr --no-index -- before.rs after.rs\n  diffr main HEAD --format ndjson\n\nUnsupported Git flags are rejected; this is not a complete git diff implementation.")
        .get_matches_from(argv);
    if opens_tui(
        args.value_source("format") == Some(clap::parser::ValueSource::CommandLine),
        args.get_flag("quiet") || args.contains_id("metadata"),
        io::stdin().is_terminal() && io::stdout().is_terminal(),
    ) {
        return launch_tui(&frontend_args);
    }
    let streaming = args.get_one::<String>("format").unwrap() == "ndjson";
    if streaming && (args.get_flag("quiet") || args.contains_id("metadata")) {
        return Err("--format ndjson cannot be combined with --quiet or metadata output".into());
    }
    let items: Vec<OsString> = args
        .get_many::<OsString>("items")
        .into_iter()
        .flatten()
        .cloned()
        .collect();
    let display = DisplayOptions {
        num_context_lines: *args.get_one::<u32>("unified").unwrap(),
        terminal_width: args
            .get_one::<usize>("width")
            .copied()
            .unwrap_or_else(crate::options::detect_terminal_width),
        use_color: !args.get_flag("no-color")
            && crate::options::should_use_color(
                match args.get_one::<String>("color").unwrap().as_str() {
                    "always" => crate::options::ColorOutput::Always,
                    "never" => crate::options::ColorOutput::Never,
                    _ => crate::options::ColorOutput::Auto,
                },
            ),
        display_mode: match args.get_one::<String>("display").unwrap().as_str() {
            "inline" => DisplayMode::Inline,
            "side-by-side-show-both" => DisplayMode::SideBySideShowBoth,
            _ => DisplayMode::SideBySide,
        },
        ..DisplayOptions::default()
    };
    let mut diff_options = DiffOptions {
        ignore_comments: args.get_flag("ignore-comments"),
        ..DiffOptions::default()
    };
    if let Some(limit) = args.get_one::<usize>("byte-limit") {
        diff_options.byte_limit = *limit;
    }
    if let Some(limit) = args.get_one::<usize>("graph-limit") {
        diff_options.graph_limit = *limit;
    }
    if let Some(limit) = args.get_one::<usize>("parse-error-limit") {
        diff_options.parse_error_limit = *limit;
    }
    if args.get_flag("no-index") {
        return no_index(
            &args,
            items.into_iter().chain(explicit_paths).collect(),
            &display,
            &diff_options,
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
    let has_changes = if args.get_flag("quiet") || args.contains_id("metadata") {
        let diff = comparison.resolve(&repo)?.diff(&repo, &files)?;
        let changed = diff.deltas().len() > 0;
        if !args.get_flag("quiet") {
            print_metadata(&diff, &args, display.terminal_width)?;
        }
        changed
    } else {
        let params = Arc::new(
            Config::load(workspace, args.get_one::<String>("config").map(Path::new))?.compile()?,
        );
        let mut session = DiffSession::open(workspace, comparison, params, &files)?;
        session.context_lines = display.num_context_lines;
        session.diff_options = diff_options;
        let changed = session.remaining() > 0;
        if streaming {
            let failed = crate::stream::write(session, &mut io::stdout().lock())?;
            return Ok(if failed {
                2
            } else {
                i32::from(changed && args.get_flag("exit-code"))
            });
        }
        for (file, result) in session {
            let result = result.map_err(|error| format!("{}: {error}", file.path()))?;
            render(&result, &args, &display)?;
        }
        changed
    };
    Ok(i32::from(
        has_changes && (args.get_flag("exit-code") || args.get_flag("quiet")),
    ))
}

fn render(
    diff: &crate::summary::DiffResult,
    args: &ArgMatches,
    display: &DisplayOptions,
) -> Result<()> {
    match args.get_one::<String>("format").unwrap().as_str() {
        "json" => println!("{}", diff.domain_json()),
        "snapshot" => print!("{}", diff.snapshot()),
        _ => crate::print_diff_result(display, diff),
    }
    Ok(())
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
    let prefix = location.strip_prefix(repo.workdir().unwrap_or(repo.path()))?;
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
    display: &DisplayOptions,
    options: &DiffOptions,
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
    let config = Config::load(
        Path::new(args.get_one::<String>("repo").unwrap()),
        args.get_one::<String>("config").map(Path::new),
    )?
    .compile()?;
    let lhs = crate::options::FileArgument::from_path_argument(&paths[0]);
    let rhs = crate::options::FileArgument::from_path_argument(&paths[1]);
    let compute = || {
        crate::diff_file(
            &config,
            &paths[1].to_string_lossy(),
            None,
            &lhs,
            &rhs,
            lhs.permissions().as_ref(),
            rhs.permissions().as_ref(),
            display,
            options,
            false,
            &[],
            &[],
        )
    };
    if args.get_one::<String>("format").map(String::as_str) == Some("ndjson") {
        crate::stream::write_file(
            &paths[0].to_string_lossy(),
            &paths[1].to_string_lossy(),
            compute,
            &mut io::stdout().lock(),
        )?;
        Ok(i32::from(changed && args.get_flag("exit-code")))
    } else {
        render(&compute(), args, display)?;
        Ok(i32::from(changed))
    }
}

/// Explicit machine/text modes and redirected output must never enter the alternate screen.
fn opens_tui(explicit_format: bool, metadata_or_quiet: bool, terminal: bool) -> bool {
    terminal && !explicit_format && !metadata_or_quiet
}

fn launch_tui(args: &[OsString]) -> Result<i32> {
    let entry = std::env::var_os("DIFFR_TUI_ENTRY")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tui/packages/hunk/src/main.tsx")
        });
    if !entry.is_file() {
        return Err(
            "Terminal frontend is unavailable; install the tui dependencies or use --format text"
                .into(),
        );
    }
    let bun = std::env::var_os("DIFFR_BUN").unwrap_or_else(|| "bun".into());
    let status = std::process::Command::new(bun)
        .arg("run").arg(entry).arg("--diffr").arg(std::env::current_exe()?)
        .arg("--").args(args).status()
        .map_err(|error| format!("Could not launch terminal frontend: {error}. Install Bun and run bun install in tui/, or use --format text."))?;
    Ok(status.code().unwrap_or(2))
}

#[cfg(test)]
mod tui_launch_tests {
    use super::opens_tui;
    #[test]
    fn only_implicit_interactive_output_opens_the_viewer() {
        assert!(opens_tui(false, false, true));
        assert!(!opens_tui(true, false, true));
        assert!(!opens_tui(false, true, true));
        assert!(!opens_tui(false, false, false));
    }
}
