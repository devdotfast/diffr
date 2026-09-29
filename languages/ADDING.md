# Moving another language into `extra`

`languages/extra.json` is the single extra-language registry. It drives package
building, generated Rust parser configuration, availability reporting, and tests.
Generated files stay under `target/`; no generated parser or upstream highlight
query or license needs to be copied into this repository.

1. Move the language's configuration from `tree_sitter_parser.rs` into an entry
   in `extra.json`. Preserve its atom nodes, delimiters, and trailing-token rules.
   Copy an existing entry with similar rules as a template.
2. Declare the grammar dependency optional in Cargo, retain a `lang-*` feature,
   and include it in `all-languages`. Both OCaml grammars share one feature.
3. Supply the pinned crate version/revision, source directory, exported C symbol,
   Rust parser constant, license path/hash, and a pair of representative fixtures.
4. Describe highlight inputs by upstream crate, Rust constant, and relative file
   path. The package builder reads the files from Cargo's locked sources; built-in
   builds use the corresponding constants. QML demonstrates combined queries.
   Use `local` only for an existing diffr customization, such as Julia or Verilog.
   An empty list preserves OCaml interfaces' existing behavior. Shared query
   dependencies include their licenses. Set each `license` to its upstream path
   and SHA-256. Packaging reads Cargo sources first; when a crate omits its
   license, it fetches from the GitHub repository at Cargo's exact VCS revision.
   Verified licenses are cached under `target/languages/licenses`; subsequent
   packaging can reuse them offline. Published archives always include licenses.
5. Run the checks below and advance the independent pack version when its contents
   change. Keep language names, globs, and detection unconditional.

Do not add another Rust or Python language list. Cargo generates the Rust registry
from `extra.json`. `check-languages` rejects duplicate built-in configurations,
missing features, license metadata, query inputs, and fixtures. Recognition, annotation
rules, and structural diff rules remain in diffr; the pack carries compiled
parser/scanner libraries and integrity-checked highlight query files.

```sh
cargo xtask check-languages
python3 languages/tests/test_definitions.py
cargo test --locked
cargo test --locked --features all-languages --bin diffr --test cli
cargo xtask build-languages --target aarch64-apple-darwin
# Set DIFFR_SIGN_IDENTITY to Whiteboard's identity for signed macOS checks.
python3 languages/tests/probe.py target/languages/aarch64-apple-darwin/package
cargo xtask package-languages --target aarch64-apple-darwin
python3 languages/tests/integration.py target/languages/aarch64-apple-darwin/catalog-entry.json --release
```

Every registry entry automatically participates in native/static structural,
syntax, tree, and installation tests. Include scanner-heavy fixtures when needed.
The tests also cover concurrent loads/installs, corrupt libraries and queries,
repair, offline use, and the platform's default store.

Publish and verify all four target archives before updating `catalog.json`, as
explained in [README.md](README.md). Existing CLIs retain their pinned revisions.
