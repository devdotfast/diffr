//! Git-style CLI input: the terminal UI, the NDJSON stream, and Git metadata.
use crate::config::{self, Config};
use crate::git::{Comparison, DiffSession, FileParams, Operand, Result};
use crate::options::DebugArgs;
use crate::params::DiffOptions;
use crate::plugin::Pipeline;
use clap::{
    error::ErrorKind, parser::ValueSource, ArgGroup, ArgMatches, Args, CommandFactory,
    FromArgMatches, Parser, Subcommand, ValueEnum,
};
use git2::{DiffStatsFormat, Repository};
use std::{
    ffi::OsString,
    io::{self, IsTerminal, Write},
    num::NonZeroUsize,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

/// The metadata outputs, named one by one in conflicts so clap's error names
/// the flag that was given rather than the whole group.
const METADATA: [&str; 5] = ["name_only", "name_status", "stat", "numstat", "shortstat"];

/// Structural diffs with Git-style comparison inputs
#[derive(Parser)]
#[command(
    name = env!("CARGO_BIN_NAME"),
    version,
    group(ArgGroup::new("metadata").args(METADATA)),
    group(ArgGroup::new("names").args(["name_only", "name_status"])),
    after_help = "Examples:\n  diffr\n  diffr --cached\n  diffr main...HEAD -- src/\n  diffr --no-index -- before.rs after.rs\n  diffr main HEAD --format ndjson\n\nUnsupported Git flags are rejected; this is not a complete git diff implementation."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
    /// Directory in the repository to diff from; paths are relative to it
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    /// Concurrent file diffs for --format ndjson; results are emitted as each finishes
    #[arg(short, long, default_value = "16")]
    jobs: NonZeroUsize,
    /// File tag priority: files carrying an earlier listed tag come first
    #[arg(long, value_delimiter = ',')]
    order: Vec<String>,
    /// Compare the index with HEAD, or with the given revision
    #[arg(long, visible_alias = "staged")]
    cached: bool,
    /// Compare from the merge base of the given revision and the second one, or HEAD
    #[arg(long)]
    merge_base: bool,
    /// Compare two files on disk rather than Git revisions
    #[arg(long, conflicts_with_all = ["cached", "merge_base", "null"], conflicts_with_all = METADATA)]
    no_index: bool,
    /// Swap the two sides of the comparison
    #[arg(short = 'R', long)]
    reverse: bool,
    /// Exit with 1 when there are differences
    #[arg(long)]
    exit_code: bool,
    /// Print nothing; exit with 1 when there are differences
    #[arg(long)]
    quiet: bool,
    /// Print the names of changed files
    #[arg(long)]
    name_only: bool,
    /// Print the names and statuses of changed files
    #[arg(long)]
    name_status: bool,
    /// Print a diffstat
    #[arg(long)]
    stat: bool,
    /// Print added and deleted line counts for each file
    #[arg(long)]
    numstat: bool,
    /// Print the diffstat's summary line
    #[arg(long)]
    shortstat: bool,
    /// Terminate --name-only and --name-status output with NULs
    #[arg(short = 'z', long, requires = "names")]
    null: bool,
    /// Do not detect renames
    #[arg(long)]
    no_renames: bool,
    /// Detect renames, which is the default
    #[arg(short = 'M', long, conflicts_with = "no_renames")]
    find_renames: bool,
    /// Unchanged lines kept around each change; defaults to plugins.bundled.context.lines
    #[arg(short = 'U', long)]
    unified: Option<u32>,
    /// Write the event stream to stdout instead of opening the terminal UI
    #[arg(long, conflicts_with = "quiet", conflicts_with_all = METADATA)]
    format: Option<Format>,
    /// Emit initial files followed by deferred annotations (NDJSON v4)
    #[arg(long, requires = "format", conflicts_with = "quiet", conflicts_with_all = METADATA)]
    stream_annotations: bool,
    /// Include every token's tree-sitter capture name in --format ndjson output
    #[arg(long, requires = "format", conflicts_with = "quiet", conflicts_with_all = METADATA)]
    syntax: bool,
    /// Columns for --stat; defaults to the terminal's width
    #[arg(long)]
    width: Option<usize>,
    /// Don't consider comments when diffing
    #[arg(long)]
    ignore_comments: bool,
    /// Files larger than this many bytes on either side get a line diff; defaults to diff.byte_limit
    #[arg(long)]
    byte_limit: Option<usize>,
    /// The largest AST matching graph to explore for one file; defaults to diff.graph_limit
    #[arg(long)]
    graph_limit: Option<usize>,
    /// Files with more parse errors than this get a line diff; defaults to diff.parse_error_limit
    #[arg(long)]
    parse_error_limit: Option<usize>,
    /// Revisions, then paths
    items: Vec<OsString>,
    /// Paths
    #[arg(last = true)]
    paths: Vec<OsString>,
}

impl Cli {
    fn metadata(&self) -> bool {
        self.name_only || self.name_status || self.stat || self.numstat || self.shortstat
    }
}

#[derive(Clone, Copy, ValueEnum)]
enum Format {
    Ndjson,
}

#[derive(Subcommand)]
enum Command {
    /// Show, edit, or open the settings screen for diffr's configuration
    Config(ConfigArgs),
    #[command(
        hide = true,
        display_name = env!("CARGO_BIN_NAME"),
        about,
        version,
        long_version = crate::version::VERSION.as_str(),
        next_display_order = None,
        arg_required_else_help = true
    )]
    Debug(DebugArgs),
}

#[derive(Args)]
struct ConfigArgs {
    /// Initial search in the settings screen
    query: Option<String>,
    #[command(subcommand)]
    command: Option<ConfigCommand>,
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Print the configuration's JSON Schema
    Schema,
    /// Print the resolved configuration
    Show {
        #[arg(long)]
        json: bool,
        /// Do not redact the API key
        #[arg(long)]
        reveal: bool,
    },
    /// Write one key to the global configuration file
    Set { key: String, value: String },
}

pub(crate) fn run() -> Result<i32> {
    let frontend_args: Vec<OsString> = std::env::args_os().skip(1).collect();
    // Git treats an argument before `--` as a revision even when a file shares
    // its name, so a bare trailing `--` still matters.
    let has_separator = frontend_args.iter().any(|arg| arg == "--");
    let matches = Cli::command().get_matches();
    reject_diff_arguments_before_subcommand(&matches);
    let args = Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit());
    match &args.command {
        Some(Command::Config(config)) => return run_config(config),
        Some(Command::Debug(debug)) => {
            crate::run_debug(debug.mode());
            return Ok(0);
        }
        None => {}
    }
    let streaming = args.format.is_some();
    let metadata_or_quiet = args.quiet || args.metadata();
    if !streaming && !metadata_or_quiet {
        return launch_tui(&frontend_args, true);
    }
    let stream_options = crate::protocol::stream::Options {
        syntax: args.syntax,
        updates: args.stream_annotations,
    };
    if args.no_index {
        return no_index(
            &args,
            args.items.iter().chain(&args.paths).cloned().collect(),
            stream_options,
        );
    }
    let location = std::fs::canonicalize(&args.repo)?;
    let repo = Repository::discover(&location)?;
    let workspace = repo.workdir().unwrap_or(repo.path());
    let (comparison, paths) = select(&repo, &location, &args, has_separator)?;
    let files = FileParams {
        paths,
        // `-M` conflicts with `--no-renames`; renames are on by default.
        renames: args.find_renames || !args.no_renames,
        order: args.order.clone(),
    };
    if metadata_or_quiet {
        let diff = comparison.resolve(&repo)?.diff(&repo, &files)?;
        let changed = diff.deltas().len() > 0;
        if !args.quiet {
            let width = args
                .width
                .unwrap_or_else(crate::options::detect_terminal_width);
            print_metadata(&diff, &args, width)?;
        }
        return Ok(i32::from(changed && (args.exit_code || args.quiet)));
    }
    let mut config = Config::load()?;
    apply_unified(&args, &mut config);
    let pipeline =
        Pipeline::from_config(&config.plugins, workspace).map_err(|error| format!("{error:#}"))?;
    let limits = config.diff;
    let params = Arc::new(config.compile_with(&pipeline)?);
    let diff_options = diff_options(&args, &limits);
    let mut session = DiffSession::open(
        workspace,
        comparison,
        Arc::clone(&params),
        &files,
        &pipeline,
    )?;
    session.diff_options = diff_options;
    let changed = session.remaining() > 0;
    let ended = crate::protocol::stream::write(
        session,
        args.jobs.get(),
        Arc::new(pipeline),
        stream_options,
        &mut io::stdout().lock(),
    )?;
    Ok(if ended.failed || ended.aborted {
        2
    } else {
        i32::from(changed && args.exit_code)
    })
}

/// The top-level flags configure a diff; a subcommand given after one would
/// silently drop it.
fn reject_diff_arguments_before_subcommand(matches: &ArgMatches) {
    let Some((name, _)) = matches.subcommand() else {
        return;
    };
    let mut command = Cli::command();
    // Formatting an `Arg` reads settings that only `build` fills in.
    command.build();
    let given = command
        .get_arguments()
        .find(|arg| matches.value_source(arg.get_id().as_str()) == Some(ValueSource::CommandLine))
        .map(ToString::to_string);
    if let Some(arg) = given {
        command
            .error(
                ErrorKind::ArgumentConflict,
                format!("the argument '{arg}' cannot be used with the '{name}' subcommand"),
            )
            .exit();
    }
}

fn select(
    repo: &Repository,
    location: &Path,
    args: &Cli,
    has_separator: bool,
) -> Result<(Comparison, Vec<String>)> {
    let mut revisions = Vec::new();
    let mut paths = Vec::new();
    for item in &args.items {
        let text = item
            .to_str()
            .ok_or("non-UTF-8 revision/path arguments are unsupported")?;
        let is_rev = repo.revparse(text).is_ok();
        let is_path = location.join(item).exists();
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
            paths.push(item.clone());
        } else {
            return Err(
                format!("unknown revision or path {text:?}; use -- before pathspecs").into(),
            );
        }
    }
    paths.extend(args.paths.iter().cloned());
    // Match canonical path forms, including Windows verbatim path prefixes.
    let root = std::fs::canonicalize(repo.workdir().unwrap_or(repo.path()))?;
    let prefix = location.strip_prefix(&root)?;
    let paths = paths
        .into_iter()
        .map(|path| normalize_path(prefix, &path))
        .collect::<Result<Vec<_>>>()?;
    let cached = args.cached;
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
    if args.merge_base {
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
    if args.reverse {
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

fn print_metadata(diff: &git2::Diff<'_>, args: &Cli, width: usize) -> Result<()> {
    let mut stdout = io::stdout().lock();
    if args.name_only || args.name_status {
        let separator: &[u8] = if args.null { b"\0" } else { b"\t" };
        let terminator: &[u8] = if args.null { b"\0" } else { b"\n" };
        for delta in diff.deltas() {
            if args.name_status {
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
    if args.numstat {
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
    let format = if args.shortstat {
        DiffStatsFormat::SHORT
    } else {
        DiffStatsFormat::FULL
    };
    stdout.write_all(&diff.stats()?.to_buf(format, width)?)?;
    Ok(())
}

fn no_index(
    args: &Cli,
    paths: Vec<OsString>,
    stream_options: crate::protocol::stream::Options,
) -> Result<i32> {
    if paths.len() != 2 {
        return Err("--no-index requires two file paths".into());
    }
    let mut paths = paths;
    if args.reverse {
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
    if args.quiet {
        return Ok(i32::from(changed));
    }
    let mut config = Config::load()?;
    apply_unified(args, &mut config);
    let pipeline = Pipeline::from_config(&config.plugins, &std::env::current_dir()?)
        .map_err(|error| format!("{error:#}"))?;
    let limits = config.diff;
    let config = config.compile_with(&pipeline)?;
    let options = &diff_options(args, &limits);
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
        i32::from(changed && args.exit_code)
    })
}

/// The engine limits: the configured `[diff]` table, then the command-line
/// flags.
fn diff_options(args: &Cli, limits: &config::DiffConfig) -> DiffOptions {
    let mut options = limits.options(args.ignore_comments);
    if let Some(limit) = args.byte_limit {
        options.byte_limit = limit;
    }
    if let Some(limit) = args.graph_limit {
        options.graph_limit = limit;
    }
    if let Some(limit) = args.parse_error_limit {
        options.parse_error_limit = limit;
    }
    options
}

/// `-U` overrides the context plugin's `lines` for this run.
fn apply_unified(args: &Cli, config: &mut Config) {
    let Some(unified) = args.unified else {
        return;
    };
    if let Some(entry) = config.plugins.entries.get_mut("bundled.context") {
        entry
            .options
            .insert("lines".into(), serde_json::Value::from(unified));
    }
}

/// `diffr config`: settings, schema, resolved values and edits.
fn run_config(config: &ConfigArgs) -> Result<i32> {
    let mut stdout = io::stdout().lock();
    match &config.command {
        Some(ConfigCommand::Schema) => {
            serde_json::to_writer_pretty(&mut stdout, &Config::schema())?;
            stdout.write_all(b"\n")?;
        }
        Some(ConfigCommand::Show { json, reveal }) => {
            let config = Config::load()?;
            if *json {
                serde_json::to_writer_pretty(&mut stdout, &config::store::show(&config, *reveal))?;
                stdout.write_all(b"\n")?;
            } else {
                stdout.write_all(
                    toml::to_string_pretty(&config::store::redacted(&config, *reveal))?.as_bytes(),
                )?;
            }
        }
        Some(ConfigCommand::Set { key, value }) => {
            config::store::set(&config::global_path()?, key, value)?;
        }
        None => {
            let mut frontend = vec![OsString::from("--settings")];
            if let Some(query) = &config.query {
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
