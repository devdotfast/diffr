use std::process::Command;

use assert_cmd::prelude::*;
use predicates::prelude::*;

fn find_runner() -> Option<String> {
    for (key, value) in std::env::vars() {
        if key.starts_with("CARGO_TARGET_") && key.ends_with("_RUNNER") && !value.is_empty() {
            return Some(value);
        }
    }
    None
}

// Sample code from
// https://github.com/assert-rs/assert_cmd/issues/139, supports
// cross-compiled binaries.
fn get_base_command() -> Command {
    let mut cmd;
    let path = assert_cmd::cargo_bin!("diffr");
    if let Some(runner) = find_runner() {
        let mut runner = runner.split_whitespace();
        cmd = Command::new(runner.next().unwrap());
        for arg in runner {
            cmd.arg(arg);
        }
        cmd.arg(path);
    } else {
        cmd = Command::new(path);
    }
    cmd
}

fn debug_command() -> Command {
    let mut cmd = get_base_command();
    cmd.arg("debug");
    cmd
}

#[test]
fn list_languages() {
    let mut cmd = debug_command();

    cmd.arg("--list-languages");

    let predicate_fn = predicate::str::contains("TOML");
    cmd.assert().stdout(predicate_fn);

    let predicate_fn = predicate::str::contains("*.toml");
    cmd.assert().stdout(predicate_fn);
}

#[test]
fn dump_tree_sitter() {
    let mut cmd = debug_command();

    cmd.arg("--dump-ts").arg("sample_files/simple_1.js");
    cmd.assert().success();
}

#[test]
fn dump_syntax() {
    let mut cmd = debug_command();

    cmd.arg("--dump-syntax").arg("sample_files/simple_1.js");
    cmd.assert().success();
}

#[test]
fn a_comparison_without_format_needs_a_terminal() {
    let mut cmd = get_base_command();

    cmd.args([
        "--no-index",
        "sample_files/simple_1.js",
        "sample_files/simple_2.js",
    ]);
    cmd.assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("--format ndjson"));
}

#[test]
fn text_output_is_gone() {
    let mut cmd = get_base_command();

    cmd.args([
        "--format",
        "text",
        "--no-index",
        "sample_files/simple_1.js",
        "sample_files/simple_2.js",
    ]);
    cmd.assert().failure();
}

#[test]
fn a_plugin_that_cannot_be_made_stops_diffr_before_any_record() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "[plugins.summarize]\nenabled = true\n").unwrap();
    let mut cmd = get_base_command();

    cmd.arg("--config")
        .arg(&config)
        .args([
            "--no-index",
            "sample_files/simple_1.js",
            "sample_files/simple_2.js",
            "--format",
            "ndjson",
        ])
        .env("XDG_CONFIG_HOME", dir.path())
        .env_remove("GEMINI_API_KEY")
        .env_remove("GOOGLE_API_KEY");
    cmd.assert()
        .failure()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "plugins.summarize: no API key: set plugins.summarize.api_key",
        ));
}
