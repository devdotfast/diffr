// Clippy errors in this file should not stop build errors being
// reported elsewhere.
// https://github.com/rust-lang/rust-clippy/issues/9534
#![warn(clippy::all)]
// Has false positives on else if chains that sometimes have the same
// body for readability.
#![allow(clippy::if_same_then_else)]

use std::path::PathBuf;
use std::process::Command;

use rayon::prelude::*;
use version_check as rustc;

struct TreeSitterParser {
    name: &'static str,
    src_dir: &'static str,
    extra_files: Vec<&'static str>,
}

impl TreeSitterParser {
    fn build(&self) {
        let dir = PathBuf::from(&self.src_dir);

        let mut c_files = vec!["parser.c"];
        c_files.extend_from_slice(&self.extra_files);

        let mut build = cc::Build::new();
        if cfg!(target_env = "msvc") {
            build.flag("/utf-8");
        }
        build.include(&dir).warnings(false); // ignore unused parameter warnings
        for file in c_files {
            build.file(dir.join(file));
        }

        build.link_lib_modifier("+whole-archive");

        build.compile(self.name);
    }
}

fn main() {
    native_plugins();
    let parsers = vec![
        TreeSitterParser {
            name: "tree-sitter-janet-simple",
            src_dir: "vendored_parsers/tree-sitter-janet-simple-src",
            extra_files: vec!["scanner.c"],
        },
        TreeSitterParser {
            name: "tree-sitter-kotlin",
            src_dir: "vendored_parsers/tree-sitter-kotlin-src",
            extra_files: vec!["scanner.c"],
        },
        TreeSitterParser {
            name: "tree-sitter-latex",
            src_dir: "vendored_parsers/tree-sitter-latex-src",
            extra_files: vec!["scanner.c"],
        },
        TreeSitterParser {
            name: "tree-sitter-smali",
            src_dir: "vendored_parsers/tree-sitter-smali-src",
            extra_files: vec!["scanner.c"],
        },
    ];

    // Only rerun if relevant files in the vendored_parsers/ directory change.
    for parser in &parsers {
        println!("cargo:rerun-if-changed={}", parser.src_dir);
    }

    parsers.par_iter().for_each(|p| p.build());
    commit_info();

    if let Some((version, _, _)) = rustc::triple() {
        println!("cargo:rustc-env=DFT_RUSTC_VERSION={}", version);
    }

    // Use 64-KiB pages with jemalloc. This solves "<jemalloc>:
    // Unsupported system page size" errors, and performs the same as
    // jemalloc's default settings.
    //
    // Note that difftastic does not use jemalloc on all operating
    // systems, but it's harmless to set this unconditionally.
    println!("cargo:rustc-env=JEMALLOC_SYS_WITH_LG_PAGE=16");
}

fn commit_info() {
    if !PathBuf::from(".git").exists() {
        return;
    }

    let output = match Command::new("git")
        .arg("log")
        .arg("-1")
        .arg("--date=short")
        .arg("--format=%H %h %cd")
        .output()
    {
        Ok(output) if output.status.success() => output,
        _ => return,
    };
    let stdout = String::from_utf8(output.stdout).unwrap();
    let mut parts = stdout.split_whitespace();
    let mut next = || parts.next().unwrap();
    let _commit_hash = next();
    println!("cargo:rustc-env=DFT_COMMIT_SHORT_HASH={}", next());
    println!("cargo:rustc-env=DFT_COMMIT_DATE={}", next())
}

/// Collect native registrations and bundled assets from plugin package metadata.
fn native_plugins() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rerun-if-changed=Cargo.toml");
    let manifest: toml::Value = std::fs::read_to_string(root.join("Cargo.toml"))
        .unwrap()
        .parse()
        .unwrap();
    let mut code = String::from("const PLUGINS: &[&sdk::Registration] = &[\n");
    let mut files = String::from("const FILES: &[(&str, &str)] = &[\n");
    let mut components = String::from("const COMPONENTS: &[(&str, &[u8])] = &[\n");
    embed_queries(
        &root.join("plugins/shared/queries"),
        "shared/queries",
        &mut files,
    );
    for (dependency, spec) in manifest["dependencies"].as_table().unwrap() {
        let Some(path) = spec.get("path").and_then(toml::Value::as_str) else {
            continue;
        };
        let file = root.join(path).join("Cargo.toml");
        println!("cargo:rerun-if-changed={}", file.display());
        let package: toml::Value = std::fs::read_to_string(file).unwrap().parse().unwrap();
        let Some(plugin) = package
            .get("package")
            .and_then(|p| p.get("metadata"))
            .and_then(|p| p.get("diffr"))
        else {
            continue;
        };
        let native = plugin
            .get("native")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false);
        let folder = root.join(path);
        let plugin_manifest = folder.join("plugin.toml");
        let description: toml::Value = std::fs::read_to_string(&plugin_manifest)
            .expect("a plugin has plugin.toml")
            .parse()
            .unwrap();
        let name = description["name"].as_str().expect("plugin name");
        println!("cargo:rerun-if-changed={}", plugin_manifest.display());
        files.push_str(&format!(
            "    ({:?}, include_str!({:?})),\n",
            format!("{name}/plugin.toml"),
            plugin_manifest
        ));
        embed_queries(
            &folder.join("queries"),
            &format!("{name}/queries"),
            &mut files,
        );
        if !native {
            let wasm = folder.join("plugin.wasm");
            println!("cargo:rerun-if-changed={}", wasm.display());
            components.push_str(&format!("    ({name:?}, include_bytes!({wasm:?})),\n"));
        }
        if native {
            code.push_str(&format!(
                "    &{}::DIFFR_PLUGIN,\n",
                dependency.replace('-', "_")
            ));
        }
    }
    code.push_str("];\n");
    files.push_str("];\n");
    components.push_str("];\n");
    files.push_str(&components);
    std::fs::write(
        PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("bundled_assets.rs"),
        files,
    )
    .unwrap();
    std::fs::write(
        PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("native_plugins.rs"),
        code,
    )
    .unwrap();
}

/// Embed query assets without maintaining a second file list in the host.
fn embed_queries(directory: &std::path::Path, prefix: &str, code: &mut String) {
    if !directory.exists() {
        println!(
            "cargo:rerun-if-changed={}",
            directory.parent().unwrap().display()
        );
        return;
    }
    println!("cargo:rerun-if-changed={}", directory.display());
    let mut entries: Vec<_> = std::fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        let name = format!("{prefix}/{}", path.file_name().unwrap().to_str().unwrap());
        if path.is_dir() {
            embed_queries(&path, &name, code);
        } else if path.extension().is_some_and(|extension| extension == "scm") {
            code.push_str(&format!("    ({name:?}, include_str!({path:?})),\n"));
        }
    }
}
