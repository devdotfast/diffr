// Clippy errors in this file should not stop build errors being
// reported elsewhere.
// https://github.com/rust-lang/rust-clippy/issues/9534
#![warn(clippy::all)]

use std::path::PathBuf;

use rayon::prelude::*;

/// A grammar that is not on crates.io, built from `vendored_parsers/`.
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
        if std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc") {
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
}

/// Collect native registrations and bundled assets from plugin package metadata.
fn native_plugins() {
    let root = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    println!("cargo:rerun-if-changed=Cargo.toml");
    let manifest: toml::Table = std::fs::read_to_string(root.join("Cargo.toml"))
        .unwrap()
        .parse()
        .unwrap();
    let mut code = String::from("const PLUGINS: &[&sdk::Registration] = &[\n");
    // The bundled components' own code, which every plugin crate also builds
    // natively: tests run it without a component runtime.
    let mut component_code =
        String::from("#[cfg(any(test, feature = \"test-support\"))]\nconst COMPONENT_CODE: &[&sdk::Registration] = &[\n");
    let mut files = String::from("const FILES: &[(&str, &str)] = &[\n");
    let mut components = String::from("const COMPONENTS: &[(&str, &[u8])] = &[\n");
    // The query files every bundled plugin imports live beside the plugins.
    embed_queries(
        &root.join("../../plugins/shared/queries"),
        "shared/queries",
        &mut files,
    );
    for dependency in manifest["dependencies"].as_table().unwrap().keys() {
        // Resolve assets through Cargo links metadata.
        if !dependency.starts_with("diffr-plugin-") || dependency == "diffr-plugin-sdk" {
            continue;
        }
        let variable = format!("DEP_{}_ASSETS", dependency.to_uppercase().replace('-', "_"));
        let folder = PathBuf::from(
            std::env::var_os(&variable)
                .unwrap_or_else(|| panic!("{dependency} did not publish {variable}")),
        );
        let file = folder.join("Cargo.toml");
        println!("cargo:rerun-if-changed={}", file.display());
        let package: toml::Table = std::fs::read_to_string(file).unwrap().parse().unwrap();
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
        let plugin_manifest = folder.join("plugin.toml");
        let description: toml::Table = std::fs::read_to_string(&plugin_manifest)
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
            component_code.push_str(&format!(
                "    &{}::DIFFR_PLUGIN,\n",
                dependency.replace('-', "_")
            ));
        }
        if native {
            code.push_str(&format!(
                "    &{}::DIFFR_PLUGIN,\n",
                dependency.replace('-', "_")
            ));
        }
    }
    code.push_str("];\n");
    component_code.push_str("];\n");
    code.push_str(&component_code);
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
