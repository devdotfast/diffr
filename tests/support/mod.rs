//! Shared test helpers.
//!
//! Spawning the built `diffr` needs care: under `cross`, the binary is built
//! for another architecture and only the runner cargo names in
//! `CARGO_TARGET_<TARGET>_RUNNER` can execute it. Every test that runs diffr
//! goes through [`diffr_command`] so it works there as well as natively.
use std::process::Command;

fn find_runner() -> Option<String> {
    for (key, value) in std::env::vars() {
        if key.starts_with("CARGO_TARGET_") && key.ends_with("_RUNNER") && !value.is_empty() {
            return Some(value);
        }
    }
    None
}

/// A command that runs the built `diffr`, through cargo's runner when there
/// is one.
///
/// Sample code from <https://github.com/assert-rs/assert_cmd/issues/139>.
pub fn diffr_command() -> Command {
    let path = assert_cmd::cargo_bin!("diffr");
    let Some(runner) = find_runner() else {
        return Command::new(path);
    };
    let mut runner = runner.split_whitespace();
    let mut cmd = Command::new(runner.next().expect("a runner names a program"));
    for arg in runner {
        cmd.arg(arg);
    }
    cmd.arg(path);
    cmd
}
