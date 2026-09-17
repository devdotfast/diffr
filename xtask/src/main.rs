//! Repository-only plugin builds. Normal diffr installation remains plain Cargo.
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;

fn cargo() -> Command {
    Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
}

fn build_plugins(root: &Path) -> Result<()> {
    let output = cargo()
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    let members = metadata["workspace_members"]
        .as_array()
        .context("workspace members")?;
    let excluded = metadata["metadata"]["diffr"]["wasm-test-exclude"].as_array();
    for package in metadata["packages"]
        .as_array()
        .context("workspace packages")?
    {
        if !members.contains(&package["id"]) || !package["metadata"]["diffr"].is_object() {
            continue;
        }
        if excluded.is_some_and(|names| names.contains(&package["name"])) {
            continue;
        }
        let name = package["name"].as_str().context("package name")?;
        let folder = Path::new(package["manifest_path"].as_str().context("manifest path")?)
            .parent()
            .context("package directory")?;
        anyhow::ensure!(
            folder.join("plugin.toml").is_file(),
            "{name}: missing plugin.toml"
        );
        eprintln!("Building WASM plugin {name}");
        let output = cargo()
            .current_dir(root)
            .args([
                "rustc",
                "--locked",
                "--release",
                "--target",
                "wasm32-wasip2",
                "--lib",
                "--crate-type",
                "cdylib",
                "--package",
                name,
                "--message-format=json",
                "--target-dir",
            ])
            .arg(root.join("target/wasm-plugins"))
            .output()?;
        if !output.status.success() {
            bail!("building {name} failed (install the target with `rustup target add wasm32-wasip2`):\n{}\n{}",
                String::from_utf8_lossy(&output.stderr), String::from_utf8_lossy(&output.stdout));
        }
        let artifact = wasm_artifact(&output.stdout, &package["id"])?;
        std::fs::copy(&artifact, folder.join("plugin.wasm"))
            .with_context(|| format!("copying {}", artifact.display()))?;
    }
    Ok(())
}

fn wasm_artifact(output: &[u8], package_id: &Value) -> Result<PathBuf> {
    for line in output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let message: Value = serde_json::from_slice(line)?;
        if message["reason"] != "compiler-artifact" || &message["package_id"] != package_id {
            continue;
        }
        for filename in message["filenames"].as_array().into_iter().flatten() {
            if let Some(path) = filename.as_str().map(PathBuf::from) {
                if path
                    .extension()
                    .is_some_and(|extension| extension == "wasm")
                {
                    return Ok(path);
                }
            }
        }
    }
    bail!("Cargo did not report a WASM artifact for {package_id}")
}

fn main() -> Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let task = std::env::args().nth(1).unwrap_or_default();
    match task.as_str() {
        "build-plugins" => build_plugins(root),
        "test-plugins" => {
            build_plugins(root)?;
            let status = cargo()
                .current_dir(root)
                .args([
                    "test",
                    "--locked",
                    "--features",
                    "wasm-plugin-tests",
                    "--test",
                    "wasm",
                ])
                .env("DIFFR_PLUGINS_BUILT", "1")
                .status()?;
            anyhow::ensure!(status.success(), "plugin tests failed");
            Ok(())
        }
        _ => bail!("usage: cargo xtask <build-plugins|test-plugins>"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selects_the_reported_artifact_for_the_requested_package() {
        let output =
            br#"{"reason":"compiler-artifact","package_id":"other","filenames":["other.wasm"]}
{"reason":"compiler-artifact","package_id":"mine","filenames":["some/custom_name.wasm"]}
"#;
        assert_eq!(
            wasm_artifact(output, &Value::from("mine")).unwrap(),
            PathBuf::from("some/custom_name.wasm")
        );
        assert!(wasm_artifact(output, &Value::from("missing")).is_err());
    }
}
