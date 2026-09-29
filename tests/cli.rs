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
fn optional_languages_are_recognized_without_compiled_parsers() {
    let listed = debug_command()
        .arg("--list-languages")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let listed = String::from_utf8(listed).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "[plugins]\norder = []\n").unwrap();

    let status = get_base_command()
        .env("DIFFR_PARSER_DIR", dir.path().join("parsers"))
        .args(["languages", "list", "--json"])
        .output()
        .unwrap();
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    let missing_code = if status["revision"].is_null() {
        "parser_unavailable"
    } else {
        "parser_not_installed"
    };
    for entry in std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/languages")).unwrap() {
        let path = entry.unwrap().path().join("language.json");
        if !path.is_file() {
            continue;
        }
        let definition: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let name = definition["id"].as_str().unwrap();
        let fixture = definition["fixture"]["prefix"].as_str().unwrap();
        let extension = definition["fixture"]["extension"].as_str().unwrap();
        let language = status["languages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|l| l["id"] == name)
            .unwrap();
        let enabled = language["availability"] == "built_in";
        assert!(listed.contains(&format!("*.{extension}")), "{name}");
        let output = get_base_command()
            .env("DIFFR_PARSER_DIR", dir.path().join("parsers"))
            .arg("--config")
            .arg(&config)
            .args([
                "--format",
                "ndjson",
                "--no-index",
                &format!("sample_files/{fixture}_1.{extension}"),
                &format!("sample_files/{fixture}_2.{extension}"),
            ])
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
            if enabled { None } else { Some(missing_code) },
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
    let config = dir.path().join("config.toml");
    std::fs::write(&config, "[plugins.bundled.summarize]\nenabled = true\n").unwrap();
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
