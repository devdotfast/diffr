# Moving another parser into `extra`

An extra parser is still a first-party diffr language. Recognition, highlight
queries, annotation queries, and diff rules remain in diffr. Only generated C
parser/scanner code moves into the native pack. Keep its existing compile-time
feature so `all-languages` remains self-contained.

## 1. Define the grammar

Create `languages/<canonical-id>/language.json`, `highlights.scm`, and `LICENSE`.
Use an existing definition such as `qml/language.json` as the template:

| Field | Meaning |
|---|---|
| `id` | Stable canonical ID, also the library filename stem |
| `variant` | Existing Rust `Language` enum variant |
| `feature` | Cargo feature enabling the built-in parser, e.g. `lang-qml` |
| `crate`, `version`, `revision`, `repository` | Locked crate and upstream source pin |
| `source` | Relative path to the generated `parser.c` and optional `scanner.c` |
| `symbol` | Exported Tree-sitter function, e.g. `tree_sitter_qmljs` |
| `fixture.prefix`, `fixture.extension` | Existing `sample_files/<prefix>_{1,2}.<extension>` pair |
| `smoke` | Small valid program that the native host must parse without errors |

Copy queries and the upstream license from the pinned source. Preserve any
existing diffr-specific highlight query. An intentionally empty query is valid;
OCaml interfaces retain their existing empty query. Scanner includes are resolved
relative to the Cargo source tree, so shared scanner headers need not be copied.

Multiple grammar entries can share a Cargo feature/dependency. OCaml and OCaml
interfaces demonstrate this. Include only grammar variants diffr actually uses;
the pack excludes F# signatures, OCaml standalone types, and Salesforce's
standalone SOQL/SOSL/log grammars.

## 2. Keep built-in and installed behavior aligned

- Make the parser dependency optional in `Cargo.toml`. Add `lang-<name>` pointing
  to `dep:<crate>` and include that feature in `all-languages`.
- Add one entry to `src/parse/optional.rs`. This single table supplies both the
  canonical ID and compile-time availability; `languages list` discovers it.
- In the existing `tree_sitter_parser.rs` arm, select the compiled grammar behind
  its feature and the already-resolved external grammar otherwise. Follow the
  QML arm. Load its own highlight query with `include_str!` from `languages/`.
  Preserve shared queries, atom rules, delimiters, and embedded-language behavior.
- Keep the `Language` variant, names, globs, and mode detection unconditional.

Do not add another language list to packaging or test scripts. They discover all
`languages/*/language.json` files. `check-languages` rejects registry/Cargo-feature
mismatches, missing `all-languages` entries, licenses, queries, and fixtures.

## 3. Validate the change

```sh
cargo xtask check-languages
python3 languages/tests/test_definitions.py
cargo test --locked
cargo test --locked --features all-languages --bin diffr --test cli
cargo xtask build-languages --target aarch64-apple-darwin
# Set DIFFR_SIGN_IDENTITY to Whiteboard's identity for the signed macOS checks.
python3 languages/tests/probe.py target/languages/aarch64-apple-darwin/package
cargo xtask package-languages --target aarch64-apple-darwin
python3 languages/tests/integration.py target/languages/aarch64-apple-darwin/catalog-entry.json --release
```

Packaging and native smoke tests automatically include every definition. The
integration suite compares every grammar's structural output, syntax spans,
parse trees, and added/deleted/unchanged files with an `all-languages` build;
it also checks parallel diffing, installation, corruption, repair, offline use,
and the platform's default store. Add scanner-heavy fixtures when the existing
pair does not cover the grammar's scanner behavior.

The generated `target/languages/<target>/test-fixtures.txt` drives the minimal
Linux runtime test; no compiler, Rust, Node, npm, or Python is present there.
The four-target workflow remains the release gate. Moving a language is not
complete until native and installed/static checks pass for that language.

## 4. Advance the pack revision

Choose a new independent version in `package.template.json` when changing pack
contents. Never reuse published bytes/version pairs. Build, strip, sign, package,
publish, and verify all four targets before updating `catalog.json`, as described
in README.md. Existing CLIs keep their own pinned revisions. Do not hand-author
hashes or temporarily trust arbitrary runtime manifests.
