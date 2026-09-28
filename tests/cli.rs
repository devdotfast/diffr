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
fn optional_languages_follow_build_features() {
    let listed = debug_command()
        .arg("--list-languages")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let listed = String::from_utf8(listed).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("diffr/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "[plugins]\norder = []\n").unwrap();

    for (name, fixture, extension, enabled) in [
        ("Apex", "apex", "trigger", cfg!(feature = "lang-apex")),
        ("Haskell", "haskell", "hs", cfg!(feature = "lang-haskell")),
        ("Julia", "julia", "jl", cfg!(feature = "lang-julia")),
        ("OCaml", "ocaml", "ml", cfg!(feature = "lang-ocaml")),
        (
            "OCaml Interface",
            "ocaml_interface",
            "mli",
            cfg!(feature = "lang-ocaml"),
        ),
        ("QML", "qml", "qml", cfg!(feature = "lang-qml")),
        ("VHDL", "vhdl", "vhd", cfg!(feature = "lang-vhdl")),
        ("Fortran", "fortran", "f90", cfg!(feature = "lang-fortran")),
        ("Verilog", "verilog", "sv", cfg!(feature = "lang-verilog")),
        ("F#", "f_sharp", "fs", cfg!(feature = "lang-fsharp")),
    ] {
        assert_eq!(listed.contains(name), enabled, "{name}");
        let output = get_base_command()
            .args([
                "--format",
                "ndjson",
                "--no-index",
                &format!("sample_files/{fixture}_1.{extension}"),
                &format!("sample_files/{fixture}_2.{extension}"),
            ])
            .env("XDG_CONFIG_HOME", dir.path())
            .assert()
            .success()
            .get_output()
            .stdout
            .clone();
        let events: Vec<serde_json::Value> = std::str::from_utf8(&output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let file = events.iter().find(|event| event["type"] == "file").unwrap();
        assert_eq!(file["diff"]["type"], "text", "{name}: {file}");
        assert_eq!(
            file["diff"]["stats"]["fallback"]["code"].as_str(),
            if enabled {
                None
            } else {
                Some("unsupported_language")
            },
            "{name}",
        );
    }
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
    let config = dir.path().join("diffr/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    std::fs::write(&config, "[plugins.bundled.summarize]\nenabled = true\n").unwrap();
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
    cmd.assert()
        .failure()
        .code(2)
        .stdout(predicate::str::is_empty())
        .stderr(predicate::str::contains(
            "plugins.bundled.summarize: no API key: set plugins.bundled.summarize.api_key",
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
