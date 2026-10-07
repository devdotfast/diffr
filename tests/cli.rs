mod support;

use std::process::Command;
use support::get_base_command;

use assert_cmd::prelude::*;
use predicates::prelude::*;

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
fn a_summarizer_without_a_key_still_diffs() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("diffr/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(
        &config,
        "[plugins.shape.bundled.summarize]\nenabled = true\n",
    )
    .unwrap();
    let mut cmd = get_base_command();

    cmd.args([
        "--no-index",
        "sample_files/simple_1.js",
        "sample_files/simple_2.js",
        "--format",
        "ndjson",
    ])
    .env("XDG_CONFIG_HOME", dir.path())
    .env_remove("GEMINI_API_KEY")
    .env_remove("GOOGLE_API_KEY");
    cmd.assert().success().stderr(predicate::str::contains(
        "summarize: off for this run: no API key",
    ));
}

#[test]
fn config_ui_requires_a_terminal() {
    let mut cmd = get_base_command();
    cmd.arg("config");
    cmd.assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("terminal UI needs a terminal"));
}

/// Invalid flag combinations are rejected while parsing, before the terminal
/// UI would launch.
#[test]
fn invalid_flag_combinations_are_rejected_before_the_terminal_ui() {
    for (args, message) in [
        (&["-z"][..], "<--name-only|--name-status>"),
        (&["--jobs", "0"], "invalid value '0' for '--jobs <JOBS>'"),
        (&["--syntax"], "--format <FORMAT>"),
        (
            &["--format", "ndjson", "--stat"],
            "cannot be used with '--stat'",
        ),
        (&["--syntax", "--stat"], "cannot be used with '--stat'"),
        (
            &["--no-index", "--cached", "a", "b"],
            "cannot be used with '--cached'",
        ),
    ] {
        get_base_command()
            .args(args)
            .assert()
            .failure()
            .code(2)
            .stderr(predicate::str::contains(message));
    }
}

#[test]
fn diff_flags_before_a_subcommand_are_rejected() {
    get_base_command()
        .args(["--cached", "config", "show"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "the argument '--cached' cannot be used with the 'config' subcommand",
        ));
}

#[test]
fn a_malformed_override_is_a_clap_error() {
    debug_command()
        .args(["--override=*.c:Nope", "--list-languages"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "invalid value '*.c:Nope' for '--override <GLOB:NAME>': no such language 'Nope'",
        ));
}

#[test]
fn a_malformed_numbered_override_names_its_variable() {
    debug_command()
        .arg("--list-languages")
        .env("DFT_OVERRIDE_2", "bad")
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "invalid value 'bad' for DFT_OVERRIDE_2: expected GLOB:LANG_NAME",
        ));
}

#[test]
fn debug_needs_exactly_one_action() {
    debug_command()
        .arg("--ignore-comments")
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains(
            "the following required arguments were not provided",
        ));
    debug_command()
        .args(["--list-languages", "--dump-ts", "sample_files/simple_1.js"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("cannot be used with"));
}
