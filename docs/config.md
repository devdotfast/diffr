# Configuration

One configuration, layered. Later layers override earlier ones, key by key:

1. Bundled defaults.
2. The global file, `$XDG_CONFIG_HOME/diffr/config.toml` (usually
   `~/.config/diffr/config.toml`). `--config PATH` replaces it and must exist.
3. The repository's `diffr.toml`, when there is one.
4. Environment variables `DIFFR_<TABLE>__<KEY>`, for example
   `DIFFR_SUMMARIZE__API_KEY=...` or `DIFFR_FOLDS__MIN_LINES=8`.
5. `--set key=value`, repeatable, for one run.

Most users need nothing beyond the global file, and `diffr config` writes it.
A repository file is for maintainers: language queries, or a rule the whole
team wants.

## Commands

```sh
diffr config                  # settings screen in the terminal frontend
diffr config summarize        # the same, searching for "summarize"
diffr config schema           # JSON Schema: description and default per key
diffr config show [--json]    # the resolved configuration; api_key redacted
diffr config show --reveal    # ...with the key
diffr config set folds.min_lines 20
diffr config set summarize.api_key "$KEY"
```

`set` writes one key into the global file (or the `--config` file), keeping
everything else in it as written. Values are TOML: `true`, `12`, `1.5`,
`["a", "b"]`; anything that is not valid TOML is taken as a string, and a
number given to a string key stays a string. Unknown keys and wrong types are
rejected before anything is written. Frontends drive their settings pages
through these three commands; the schema is the only contract.

## Keys

```toml
[folds]
min_lines = 12             # bodies shorter than this are never collapsed by a rule
collapse_deleted = true    # deleted function bodies start collapsed, header visible
collapse_test_bodies = true # test bodies start collapsed on both sides, header visible
collapse_removed_lines = 5 # removed stretches this long, in unpaired code, collapse in the middle; 0 disables
collapse_generated = true  # generated files start hidden
collapse_tests = true      # test files start hidden
context_lines = 3          # unchanged lines kept around a change; -U overrides

[summarize]
enabled = true             # off without an API key, silently
provider = "gemini"
model = "gemini-3.8-flash"
min_lines = 20             # new function bodies shorter than this are shown as code
api_key = "..."            # or GEMINI_API_KEY / GOOGLE_API_KEY in the environment
endpoint = "https://..."   # optional base URL override, for proxies and tests
timeout_ms = 60000
max_concurrency = 16       # requests in flight across files
retries = 3                # on timeouts, rate limits and server errors

[diff]
byte_limit = 1000000       # larger files on either side get a line diff
graph_limit = 3000000      # the largest AST matching graph explored per file
parse_error_limit = 0      # more tree-sitter parse errors than this: line diff

[theme]
name = "default-dark"      # a bundled terminal theme
path = "/path/to/theme.toml"  # or a Helix-style theme file

[languages.rust]           # tree-sitter queries; see src/config/README.md
folds = '''...'''
context = '''...'''

[folds.hook]               # an external JSON-RPC summarizer; see streaming.md
command = ["uv", "run", "--script", "examples/hooks/summarize.py"]
tags = ["body"]
min_lines = 12             # optional; defaults to folds.min_lines
timeout_ms = 5000
startup_timeout_ms = 30000
```

`languages` and `folds.hook` are not in the schema, so the settings screen
does not show them; `config set` still accepts their keys.

When a file exceeds a `[diff]` limit it falls back to a line diff: no folds,
no collapse rules, no summaries, and the file's `stats` carries a `fallback`
with code `too_large`, `too_complex` or `parse_error` and a message naming
the key to raise. `DFT_BYTE_LIMIT`, `DFT_GRAPH_LIMIT` and
`DFT_PARSE_ERROR_LIMIT` override the file for one run, and `--byte-limit`,
`--graph-limit` and `--parse-error-limit` override both. The defaults are
difftastic's. A large rewrite can exceed the graph limit and fall back to a
line diff; raising the limit trades memory for it across every file diffed in
parallel, so prefer narrowing the comparison or leaving the fallback.

## File categories

Every file in the manifest carries a `category`: `source`, `test`,
`generated`, `docs`, or whatever a repository assigns. In order of precedence:

1. A `diffr-classify` git attribute.
2. A set `linguist-generated` attribute, which means `generated`.
3. Built-in path rules: lockfiles and `dist/`, `build/`, `vendor/`,
   `node_modules/`, `__generated__/`, `*.min.js`, `*.pb.go`, `*.generated.*`
   are generated; `tests/`, `test/`, `__tests__/`, `spec/`, `*_test.go`,
   `*.test.*`, `*.spec.*`, `test_*.py`, `*_test.py`, `conftest.py`, `tests.rs`,
   `test.rs`, `*_test.rs`, `*_tests.rs` are tests;
   `docs/` and `*.md` are docs.

```gitattributes
web/schema.json  diffr-classify=generated
fixtures/**      diffr-classify=test
```

## Mutations

After each file is diffed and projected onto the wire, mutations adjust what
starts collapsed and what the collapsed label says. They never touch the diff
itself. In order:

1. `collapse_generated`, `collapse_tests`: the file's `visibility` in the
   manifest, before `start` is written.
2. `collapse_deleted`: deleted function bodies (folds tagged `function`) of
   at least `min_lines` lines, labelled `"<n> lines removed"`. A body is
   deleted only when nothing under it is paired with the after side; a
   function whose header moved but whose lines still align is a rewrite
   and stays open.
3. `collapse_test_bodies`: bodies of test functions (folds tagged `test`) of
   three or more lines, on both sides, labelled `"test body"`. A whole test
   module such as a Rust `#[cfg(test)] mod tests` is one `test` fold too,
   labelled `"test module"`, with the test bodies still foldable inside it.
4. `collapse_removed_lines`: removed stretches with no counterpart on the
   after side and at least that many lines keep their first and last line
   open and collapse the middle, tagged `removed` and labelled
   `"<n> lines removed"`. Only unpaired code qualifies: the nearest
   enclosing `function` fold (or, outside any function, the nearest
   enclosing fold) must itself have nothing paired under it, so a rewritten
   function shows its removed lines in place. Stretches at the top level
   always qualify. Stretches under a fold that already starts collapsed are
   left alone.
5. The built-in summarizer: new function bodies (folds tagged `function`,
   never a test, never one nested inside another selected body) of at least
   `summarize.min_lines` lines on the after side become python-flavored
   pseudocode. A summary with more than half the body's non-blank lines is
   discarded and the body stays open. The label starts with a comment line
   in the file's own syntax, `# pseudocode` or `// pseudocode`, then the
   text.
6. `folds.hook`, when configured, over the same selection with its own tags
   and threshold; its text gets the same comment line.
7. Grouping, always on: adjacent context gaps merge into one, and a run of
   two or more sibling folds that start collapsed is wrapped in one `group`
   fold labelled `"<n> functions removed"`, `"<n> functions summarized"`, or
   `"<n> folded regions"`. Expanding it reveals each child's own row.

A mutation that fails after its retries ends the run: the stream finishes with
`complete.aborted` (`summarizer_failed` or `hook_failed`) and diffr exits 2.
Files already written stay valid.
