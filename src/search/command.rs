use super::{configured_session, input, pprint, Options, Scope, SearchResult};
use anyhow::{bail, Context};
use clap::{Args, Subcommand};
use std::{
    io::{self, Read, Write},
    path::PathBuf,
};

/// `diffr search BASE HEAD`, or `diffr search pprint`.
#[derive(Args)]
#[command(args_conflicts_with_subcommands = true, subcommand_negates_reqs = true)]
pub(crate) struct SearchCommand {
    #[command(subcommand)]
    step: Option<Step>,
    /// Repository containing both commits; defaults to the current directory
    #[arg(long, default_value = ".")]
    repo: PathBuf,
    /// Base commit or revision
    #[arg(required = true)]
    base: Option<String>,
    /// Head commit or revision
    #[arg(required = true)]
    head: Option<String>,
    /// JSON file containing per-call plugin overrides, e.g. {"plugins":{"order":[]}}
    #[arg(long)]
    options: Option<PathBuf>,
}

#[derive(Subcommand)]
enum Step {
    /// Pretty-print search results from stdin without repository access
    Pprint,
}

pub(crate) fn run(
    command: &SearchCommand,
    runtime: &tokio::runtime::Runtime,
) -> anyhow::Result<()> {
    match (&command.step, &command.base, &command.head) {
        (Some(Step::Pprint), ..) => pprint(),
        (None, Some(base), Some(head)) => search(command, base, head, runtime),
        (None, ..) => bail!("give BASE and HEAD, or pprint"),
    }
}

/// Read revision-qualified Git-grep hits from stdin and write search results.
fn search(
    command: &SearchCommand,
    base: &str,
    head: &str,
    runtime: &tokio::runtime::Runtime,
) -> anyhow::Result<()> {
    let mut bytes = Vec::new();
    io::stdin().lock().read_to_end(&mut bytes)?;
    let hits = input::parse(&bytes)?;
    let scope = Scope::resolve(&command.repo, base, head)?;
    let options = match &command.options {
        Some(path) => serde_json::from_reader(std::fs::File::open(path)?)
            .context("invalid search options JSON")?,
        None => Options::default(),
    };
    let results = if hits.is_empty() {
        Vec::new()
    } else {
        configured_session(scope, options)?.search(hits, runtime)?
    };
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, &results)?;
    writeln!(stdout)?;
    Ok(())
}

fn pprint() -> anyhow::Result<()> {
    let results: Vec<SearchResult> = serde_json::from_reader(io::stdin().lock())
        .context("expected a JSON array of search results")?;
    let rendered = results
        .iter()
        .map(pprint::render)
        .collect::<anyhow::Result<Vec<_>>>()?;
    let mut stdout = io::stdout().lock();
    if !rendered.is_empty() {
        writeln!(stdout, "{}", rendered.join("\n\n"))?;
    }
    Ok(())
}
