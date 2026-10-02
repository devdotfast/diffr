//! What the CLI's plugins reach outside diffr through ([`Environment`]): the
//! `git` on `PATH`, Wasmtime for component plugins, and the system clock.
use super::host::Git;
use super::wasm::Wasmtime;
use super::Environment;
use anyhow::Context as _;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The CLI's environment for plugins working in `workdir`.
pub fn environment<'a>(workdir: &'a Path, components: &'a Wasmtime) -> Environment<'a> {
    Environment {
        workdir,
        git: Arc::new(ProcessGit),
        components: Some(components),
        stopwatch: Some(stopwatch),
    }
}

fn stopwatch() -> Box<dyn FnOnce() -> Duration> {
    let started = Instant::now();
    Box::new(move || started.elapsed())
}

/// Runs `git` as a child process.
pub struct ProcessGit;

impl Git for ProcessGit {
    fn run(&self, workdir: &Path, args: &[String]) -> anyhow::Result<Result<String, String>> {
        let output = Command::new("git")
            .args(args)
            .current_dir(workdir)
            .output()
            .context("git could not be started")?;
        let text = |bytes: Vec<u8>| String::from_utf8(bytes).context("git wrote non-UTF-8");
        Ok(match output.status.success() {
            true => Ok(text(output.stdout)?),
            false => Err(text(output.stderr)?),
        })
    }
}
