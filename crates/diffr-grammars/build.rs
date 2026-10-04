//! Link the pinned grammars with their parse tables compressed.
//!
//! Tree-sitter generates each grammar as one `parser.c`: constant tables plus a
//! lexer. This script moves every table that holds no pointers out of
//! `parser.c` into zstd byte arrays and leaves empty arrays in their place. A generated C function, `diffr_unpack_<symbol>`, fills them, and
//! `src/lib.rs` runs it once before the grammar's first use.
use rayon::prelude::*;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Command;

// Grammar crates: (constructor symbol, crate name, source folder, required feature).
const PACKAGES: &[(&str, &str, &str, Option<&str>)] = &[
    ("tree_sitter_go", "tree-sitter-go", "src", None),
    (
        "tree_sitter_javascript",
        "tree-sitter-javascript",
        "src",
        None,
    ),
    ("tree_sitter_python", "tree-sitter-python", "src", None),
    (
        "tree_sitter_rust_orchard",
        "tree-sitter-rust-orchard",
        "src",
        None,
    ),
    (
        "tree_sitter_typescript",
        "tree-sitter-typescript",
        "typescript/src",
        None,
    ),
    ("tree_sitter_tsx", "tree-sitter-typescript", "tsx/src", None),
    ("tree_sitter_ada", "tree-sitter-ada", "src", None),
    (
        "tree_sitter_apex",
        "tree-sitter-sfapex",
        "apex/src",
        Some("lang-apex"),
    ),
    ("tree_sitter_asm", "tree-sitter-asm", "src", None),
    ("tree_sitter_bash", "tree-sitter-bash", "src", None),
    ("tree_sitter_c", "tree-sitter-c", "src", None),
    ("tree_sitter_c_sharp", "tree-sitter-c-sharp", "src", None),
    (
        "tree_sitter_clojure_orchard",
        "tree-sitter-clojure-orchard",
        "src",
        None,
    ),
    ("tree_sitter_cmake", "tree-sitter-cmake", "src", None),
    (
        "tree_sitter_commonlisp",
        "tree-sitter-commonlisp",
        "src",
        None,
    ),
    (
        "tree_sitter_containerfile",
        "tree-sitter-containerfile",
        "src",
        None,
    ),
    ("tree_sitter_cpp", "tree-sitter-cpp", "src", None),
    ("tree_sitter_css", "tree-sitter-css", "src", None),
    (
        "tree_sitter_dart_orchard",
        "tree-sitter-dart-orchard",
        "src",
        None,
    ),
    (
        "tree_sitter_devicetree",
        "tree-sitter-devicetree",
        "src",
        None,
    ),
    ("tree_sitter_elisp", "tree-sitter-elisp", "src", None),
    ("tree_sitter_elixir", "tree-sitter-elixir", "src", None),
    ("tree_sitter_elm", "tree-sitter-elm", "src", None),
    ("tree_sitter_erlang", "tree-sitter-erlang", "src", None),
    ("tree_sitter_fish", "tree-sitter-fish", "src", None),
    (
        "tree_sitter_fortran",
        "tree-sitter-fortran",
        "src",
        Some("lang-fortran"),
    ),
    (
        "tree_sitter_fsharp",
        "tree-sitter-fsharp",
        "fsharp/src",
        Some("lang-fsharp"),
    ),
    ("tree_sitter_gleam", "tree-sitter-gleam", "src", None),
    (
        "tree_sitter_haskell",
        "tree-sitter-haskell",
        "src",
        Some("lang-haskell"),
    ),
    ("tree_sitter_hcl", "tree-sitter-hcl", "src", None),
    ("tree_sitter_html", "tree-sitter-html", "src", None),
    (
        "tree_sitter_java_orchard",
        "tree-sitter-java-orchard",
        "src",
        None,
    ),
    ("tree_sitter_json", "tree-sitter-json", "src", None),
    (
        "tree_sitter_julia",
        "tree-sitter-julia",
        "src",
        Some("lang-julia"),
    ),
    ("tree_sitter_lua", "tree-sitter-lua", "src", None),
    ("tree_sitter_make", "tree-sitter-make", "src", None),
    ("tree_sitter_newick", "tree-sitter-newick", "src", None),
    ("tree_sitter_nix", "tree-sitter-nix", "src", None),
    ("tree_sitter_objc", "tree-sitter-objc", "src", None),
    (
        "tree_sitter_ocaml",
        "tree-sitter-ocaml",
        "grammars/ocaml/src",
        Some("lang-ocaml"),
    ),
    (
        "tree_sitter_ocaml_interface",
        "tree-sitter-ocaml",
        "grammars/interface/src",
        Some("lang-ocaml"),
    ),
    ("tree_sitter_pascal", "tree-sitter-pascal", "src", None),
    ("tree_sitter_perl", "ts-parser-perl", "src", None),
    ("tree_sitter_php", "tree-sitter-php", "php/src", None),
    ("tree_sitter_proto", "tree-sitter-proto", "src", None),
    (
        "tree_sitter_qmljs",
        "tree-sitter-qmljs",
        "src",
        Some("lang-qml"),
    ),
    ("tree_sitter_r", "tree-sitter-r", "src", None),
    ("tree_sitter_racket", "tree-sitter-racket", "src", None),
    ("tree_sitter_ruby", "tree-sitter-ruby", "src", None),
    ("tree_sitter_scala", "tree-sitter-scala", "src", None),
    ("tree_sitter_scheme", "tree-sitter-scheme", "src", None),
    ("tree_sitter_solidity", "tree-sitter-solidity", "src", None),
    ("tree_sitter_sql", "tree-sitter-sequel", "src", None),
    ("tree_sitter_swift", "tree-sitter-swift", "src", None),
    ("tree_sitter_toml", "tree-sitter-toml-ng", "src", None),
    (
        "tree_sitter_verilog",
        "tree-sitter-verilog",
        "src",
        Some("lang-verilog"),
    ),
    (
        "tree_sitter_vhdl",
        "tree-sitter-vhdl",
        "src",
        Some("lang-vhdl"),
    ),
    ("tree_sitter_xml", "tree-sitter-xml", "xml/src", None),
    ("tree_sitter_yaml", "tree-sitter-yaml", "src", None),
    ("tree_sitter_zig", "tree-sitter-zig", "src", None),
];

// Grammars in vendored_parsers: (constructor symbol, folder).
const VENDORED: &[(&str, &str)] = &[
    ("tree_sitter_janet_simple", "tree-sitter-janet-simple-src"),
    ("tree_sitter_kotlin", "tree-sitter-kotlin-src"),
    ("tree_sitter_latex", "tree-sitter-latex-src"),
    ("tree_sitter_smali", "tree-sitter-smali-src"),
];

fn main() {
    // Table bytes are dumped by a host program and used on the target.
    let target_endian = std::env::var("CARGO_CFG_TARGET_ENDIAN").unwrap();
    assert_eq!(
        target_endian,
        if cfg!(target_endian = "little") {
            "little"
        } else {
            "big"
        },
        "packed grammar tables need the host and target to share byte order"
    );
    let roots = package_roots();
    let mut sources: Vec<(&str, PathBuf)> = PACKAGES
        .iter()
        .filter(|(_, _, _, feature)| {
            feature.is_none_or(|feature| {
                let name = feature.replace('-', "_").to_uppercase();
                std::env::var_os(format!("CARGO_FEATURE_{name}")).is_some()
            })
        })
        .map(|&(symbol, package, folder, _)| (symbol, roots[package].join(folder)))
        .collect();
    sources.extend(
        VENDORED
            .iter()
            .map(|&(symbol, folder)| (symbol, Path::new("../../vendored_parsers").join(folder))),
    );
    for (_, source) in &sources {
        println!("cargo:rerun-if-changed={}", source.display());
    }
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    sources
        .par_iter()
        .for_each(|(symbol, source)| pack(symbol, source, &out.join(symbol)));
    let mut grammars = String::new();
    for (symbol, _) in &sources {
        let name = symbol.strip_prefix("tree_sitter_").unwrap().to_uppercase();
        writeln!(
            grammars,
            "grammar!({name}, diffr_unpack_{symbol}, diffr_{symbol});"
        )
        .unwrap();
    }
    std::fs::write(out.join("grammars.rs"), grammars).unwrap();
}

/// Map each locked package name to its source directory, including vendored registries.
fn package_roots() -> BTreeMap<String, PathBuf> {
    let metadata = Command::new(std::env::var_os("CARGO").unwrap())
        .args([
            "metadata",
            "--locked",
            "--all-features",
            "--format-version",
            "1",
        ])
        .output()
        .expect("reading Cargo dependency metadata");
    assert!(
        metadata.status.success(),
        "{}",
        String::from_utf8_lossy(&metadata.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&metadata.stdout).unwrap();
    metadata["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|package| {
            let manifest = Path::new(package["manifest_path"].as_str().unwrap());
            (
                package["name"].as_str().unwrap().to_owned(),
                manifest.parent().unwrap().to_owned(),
            )
        })
        .collect()
}

/// Write `<out>/grammar.c` with packed tables, the `diffr_unpack_<symbol>` function that
/// fills them, and the grammar's constructor renamed to `diffr_<symbol>`, then compile it.
fn pack(symbol: &str, source: &Path, out: &Path) {
    std::fs::create_dir_all(out).unwrap();
    let parser = std::fs::read_to_string(source.join("parser.c")).unwrap();
    let tables = tables(&parser);
    let bytes = dump_tables(symbol, source, out, &parser, &tables);

    let mut grammar = format!(
        "#include <stdlib.h>\n\
         size_t ZSTD_decompress(void *dst, size_t dst_capacity, const void *src, size_t src_size);\n\
         #define {symbol} diffr_{symbol}\n"
    );
    grammar.push_str(&empty_tables(&parser, &tables, &bytes));
    for (table, bytes) in tables.iter().zip(&bytes) {
        let packed = zstd::bulk::compress(bytes, 19).unwrap();
        write!(
            grammar,
            "\nstatic const unsigned char packed_{}[] = {{",
            table.name
        )
        .unwrap();
        for byte in packed {
            write!(grammar, "{byte},").unwrap();
        }
        grammar.push_str("};\n");
    }
    grammar.push_str(
        "\nstatic void unpack(void *table, size_t size, const unsigned char *packed, size_t packed_size) {\n\
         \x20 if (ZSTD_decompress(table, size, packed, packed_size) != size) abort();\n\
         }\n",
    );
    writeln!(grammar, "\nvoid diffr_unpack_{symbol}(void) {{").unwrap();
    for Table { name, .. } in &tables {
        writeln!(
            grammar,
            "  unpack({name}, sizeof {name}, packed_{name}, sizeof packed_{name});"
        )
        .unwrap();
    }
    grammar.push_str("}\n");
    std::fs::write(out.join("grammar.c"), grammar).unwrap();

    let mut build = cc::Build::new();
    build
        .include(source)
        .warnings(false)
        .out_dir(out)
        .file(out.join("grammar.c"));
    if std::env::var("CARGO_CFG_TARGET_ENV").unwrap() == "msvc" {
        build.flag("/utf-8");
    }
    if source.join("scanner.c").is_file() {
        build.file(source.join("scanner.c"));
    }
    build.compile(symbol);
}

/// Compile and run a host program that writes each table's bytes.
fn dump_tables(
    symbol: &str,
    source: &Path,
    out: &Path,
    parser: &str,
    tables: &[Table],
) -> Vec<Vec<u8>> {
    let mut dump = String::from("#include <stdio.h>\n#include \"parser.c\"\n");
    // The constructor refers to the external scanner; the dump never calls it.
    if parser.contains(&format!("{symbol}_external_scanner_create")) {
        write!(
            dump,
            "void *{symbol}_external_scanner_create(void) {{ return NULL; }}\n\
             void {symbol}_external_scanner_destroy(void *p) {{ (void)p; }}\n\
             bool {symbol}_external_scanner_scan(void *p, TSLexer *l, const bool *v) {{ (void)p; (void)l; (void)v; return false; }}\n\
             unsigned {symbol}_external_scanner_serialize(void *p, char *b) {{ (void)p; (void)b; return 0; }}\n\
             void {symbol}_external_scanner_deserialize(void *p, const char *b, unsigned n) {{ (void)p; (void)b; (void)n; }}\n"
        )
        .unwrap();
    }
    // Write the tables back to back into argv[1] and print each one's size.
    dump.push_str(
        "int main(int argc, char **argv) {\n  (void)argc;\n  FILE *file = fopen(argv[1], \"wb\");\n  if (!file) return 1;\n",
    );
    for Table { name, .. } in tables {
        writeln!(
            dump,
            "  if (fwrite({name}, 1, sizeof {name}, file) != sizeof {name}) return 1;\n  \
             printf(\"%zu\\n\", sizeof {name});"
        )
        .unwrap();
    }
    dump.push_str("  return fclose(file);\n}\n");
    let dump_c = out.join("dump.c");
    std::fs::write(&dump_c, dump).unwrap();

    let host = std::env::var("HOST").unwrap();
    let tool = cc::Build::new()
        .host(&host)
        .target(&host)
        .opt_level(0)
        .debug(false)
        .warnings(false)
        .cargo_metadata(false)
        .include(source)
        .get_compiler();
    let program = out
        .join("dump")
        .with_extension(std::env::consts::EXE_EXTENSION);
    let mut command = tool.to_command();
    if tool.is_like_msvc() {
        command
            .arg(&dump_c)
            .arg(format!("/Fe{}", program.display()))
            .arg(format!("/Fo{}\\", out.display()));
    } else {
        command.arg(&dump_c).arg("-o").arg(&program);
    }
    assert!(
        command.status().unwrap().success(),
        "compiling the {symbol} table dump"
    );
    let file = out.join("tables.bin");
    let output = Command::new(&program).arg(&file).output().unwrap();
    assert!(output.status.success(), "dumping the {symbol} tables");
    let mut bytes = std::fs::read(&file).unwrap();
    let mut tables = Vec::new();
    for size in String::from_utf8(output.stdout).unwrap().lines() {
        let rest = bytes.split_off(size.trim().parse().unwrap());
        tables.push(std::mem::replace(&mut bytes, rest));
    }
    assert!(
        bytes.is_empty(),
        "the {symbol} table dump has trailing bytes"
    );
    tables
}

/// A top-level array definition in `parser.c`, `T name[...] = {...};`, whose elements hold no pointers.
struct Table<'a> {
    name: &'a str,
    element: &'a str,
    /// The array declarator when it states its size; `None` for `name[]`.
    sized_declarator: Option<&'a str>,
    definition: Range<usize>,
}

/// Find the tables to pack. Arrays of pointers, such as `ts_symbol_names`, parse
/// with a pointer declarator and are left in place.
fn tables(parser: &str) -> Vec<Table<'_>> {
    let mut c = tree_sitter::Parser::new();
    c.set_language(&tree_sitter_c::LANGUAGE.into()).unwrap();
    let tree = c.parse(parser, None).unwrap();
    let text = |node: tree_sitter::Node| &parser[node.byte_range()];
    let mut cursor = tree.walk();
    tree.root_node()
        .children(&mut cursor)
        .filter_map(|definition| {
            let init = definition
                .child_by_field_name("declarator")
                .filter(|init| {
                    definition.kind() == "declaration" && init.kind() == "init_declarator"
                })?;
            init.child_by_field_name("value")
                .filter(|value| value.kind() == "initializer_list")?;
            let array = init
                .child_by_field_name("declarator")
                .filter(|array| array.kind() == "array_declarator")?;
            let mut name = array;
            while name.kind() == "array_declarator" {
                name = name.child_by_field_name("declarator").unwrap();
            }
            (name.kind() == "identifier").then_some(())?;
            Some(Table {
                name: text(name),
                element: text(definition.child_by_field_name("type").unwrap()),
                sized_declarator: array.child_by_field_name("size").map(|_| text(array)),
                definition: definition.byte_range(),
            })
        })
        .collect()
}

/// Replace each table's definition with an empty, writable array of the same size.
fn empty_tables(parser: &str, tables: &[Table], bytes: &[Vec<u8>]) -> String {
    let mut emptied = parser.to_owned();
    for (table, bytes) in tables.iter().zip(bytes).rev() {
        let declarator = table.sized_declarator.map_or_else(
            || {
                format!(
                    "{}[{} / sizeof({})]",
                    table.name,
                    bytes.len(),
                    table.element
                )
            },
            str::to_owned,
        );
        emptied.replace_range(
            table.definition.clone(),
            &format!("static {} {declarator};", table.element),
        );
    }
    emptied
}
