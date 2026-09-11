# CLI diff streaming

```sh
diffr main HEAD --format ndjson
diffr --cached --format ndjson --syntax --order source,test -- src/
diffr --no-index --format ndjson -- before.rs after.rs
```

Spawn one process per comparison and consume stdout line by line. The comparison
arguments are the same as the [ordinary CLI](cli.md). There is no HTTP server.
`--format json` remains the bare domain-object output; `ndjson` is the stream
described here. `ndjson-v1` is the previous stream and will be removed once every
frontend reads this one. NDJSON requires a repository comparison or `--no-index`;
it rejects `--quiet` and metadata output flags.

The Rust types behind this document are `src/protocol/mod.rs`; the projection
from the internal diff is `src/protocol/project.rs`.

## Configuration and ordering

Each invocation resolves one configuration from the bundled defaults, the
global file, the repository's `diffr.toml`, `DIFFR_*` variables and `--set`
overrides, then compiles it once; see [config.md](config.md). Omitted keys
retain defaults; query strings replace whole values, and empty queries disable
that feature.

File categories come from `diffr-classify` and `linguist-generated` Git
attributes, then built-in path rules for lockfiles, build output, tests and
docs:

```gitattributes
*          diffr-classify=source
tests/**   diffr-classify=test
docs/**    diffr-classify=docs
**/*.lock  diffr-classify=generated
```

`--order source,test,docs` prioritizes those categories. It can also be repeated.
Unlisted and unclassified files follow, with path order breaking ties. Without
`--order`, use path order. Renames classify by new path; deletions by old path.

Paths after `--` accept libgit2 directory prefixes and wildcard patterns, not Git
magic pathspecs. Only changed files are selected. An unmatched path yields an empty
stream. Filtering precedes rename detection, so selecting one side of a rename
can appear as an addition or deletion. `--no-renames` skips rename detection.

## Conventions

Stdout carries UTF-8 newline-delimited JSON, one record per line. Read complete
lines; pipe reads can split records or contain several.

- Every enum is tagged: `type` on records, snapshots and diffs; `kind` on regions.
  Tags and enum values are `snake_case`.
- Optional, empty, and default fields are omitted, never `null`. Read a missing
  `visibility` as open, a missing `tags` as none, a missing `changed` as none.
- Consumers ignore unknown fields and tolerate unknown enum strings. Additions
  within a `version` never change the meaning of existing fields.
- Sides are `lhs` (before) and `rhs` (after). Anything that can exist on one side
  only is a *pairing*, written by presence: `{"lhs": …, "rhs": …}`, `{"lhs": …}`,
  or `{"rhs": …}`.
- Lines are 0-based and split on `\n` only: an empty file has zero lines, and a
  file without a trailing newline still counts its last line. Columns are 0-based
  byte offsets into the UTF-8 text on the wire. Ranges are half-open.

## Records

| Record | Fields | Consumer action |
| --- | --- | --- |
| `start` | `version: 2`, `lhs`, `rhs`, `files` | Lay out every file up front. |
| `file` | `file`, then `diff` or `error` | Render a result, or mark the file failed. |
| `complete` | `succeeded`, `failed`, optional `aborted` | Mark complete; `aborted` means the run stopped early. |

`lhs` and `rhs` on `start` say what is being compared: `{"type":"revision","rev":"<oid>"}`,
`{"type":"index"}`, `{"type":"working_tree"}`, `{"type":"empty_tree"}` for an unborn
branch, or `{"type":"path","path":"…"}` for `--no-index`. Revisions resolve once.
Index content is pinned by blob id during discovery; working-tree files are read
as results are computed, not as an atomic snapshot.

`files` lists every selected file in priority order:

```jsonc
{"file": {"lhs": {"path": "src/a.rs", "oid": "3b18…", "mode": "100644"},
          "rhs": {"path": "src/a.rs", "oid": "9be2…", "mode": "100644"}},
 "status": "modified",            // added | deleted | modified | renamed | copied | type_changed
 "category": "source",            // from diffr-classify; absent when unset
 "language": "Rust",              // guessed from the path; absent when unknown
 "visibility": {"collapsed": true, "label": "Generated file"}}   // absent when open
```

`file` is git's delta: a deleted file has `lhs` only, an added file `rhs` only. The
path pair is the file's identity; each `file` record repeats it verbatim so the
record can be matched back to the manifest. Working-tree sides carry git's
all-zero oid. A `--no-index` comparison has empty `oid` and `mode`.

Results arrive in completion order, not manifest order, since files are diffed
concurrently (`--jobs`, default 16). `--jobs 1` restores priority order. At
completion, `succeeded + failed` equals the manifest length unless `aborted` is
present. EOF without `complete` means the output was cut off.

### Errors

One shape everywhere: `{"code": "<snake_case>", "message": "<prose>"}`.

- On a `file` record, `error` replaces `diff` and the run continues. Codes:
  `binary`, `not_utf8`, `unsupported_file_type` (symlinks, submodules),
  `unmerged`, `read_failed`, `fold_pairing`.
- On `complete`, `aborted` reports a run-level failure: `summarizer_failed` or
  `hook_failed`. diffr stops pulling files, lets the ones in flight finish, and
  exits 2. Every `file` record already written stays valid.
- Setup failures (bad revision, unreadable config, hook that never starts)
  write to stderr and exit 2 before any record.

Exit status is 0 on success, 1 with `--exit-code` when there are changes, 2 when
any file failed or the run aborted.

## The diff

```jsonc
{"type": "text",
 "lhs": {"text": "…", "syntax": [...], "regions": [...]},
 "rhs": {"text": "…", "syntax": [...], "regions": [...]},
 "stats": {"textual": {"added": 4, "removed": 1},
           "structural": {"added": 3, "removed": 1}}}      // or "fallback": {code, message}
```

A `binary` diff carries only `{"lhs": {"size": n}, "rhs": {"size": n}}`; either
side being binary makes the whole diff binary. Text sides are a pairing too: a
deleted file has `lhs` only.

`text` is the complete source. `syntax` is present only with `--syntax`: every
token as `{line, start_column, end_column, capture}`, where `capture` is the
tree-sitter highlight capture name (`keyword`, `function.method`, …). Spans are
per line, sorted, and non-overlapping; where captures nest, the innermost wins.
Files that fell back to a line diff have no syntax.

`stats.textual` counts lines with any byte change. `stats.structural` counts lines
with a syntactic change and is replaced by `fallback` when the AST match did not
run: `unsupported_language`, `too_large`, `too_complex`, `parse_error`. A fallback
diff is aligned by a line diff and its `changed` spans are word-level, but the
parse still stands: folds and the enclosing-header context are present whenever
the language parsed (`too_complex`, `parse_error`), paired through that alignment
exactly as they are for a structural diff. Only `unsupported_language` and
`too_large` produce leaves alone.

### Regions

Each side carries a tree of regions. A region is a line range on that side with
an `id`, and the same `id` on the other side marks its counterpart.

```jsonc
{"id": 7, "kind": "fold",
 "start": {"line": 18, "column": 4}, "end": {"line": 52, "column": 33},
 "tags": ["body"],
 "visibility": {"collapsed": false, "label": "Body"},
 "children": [
   {"id": 8, "kind": "leaf", "start": {"line": 18, "column": 0}, "end": {"line": 30, "column": 0}},
   {"id": 9, "kind": "leaf", "start": {"line": 30, "column": 0}, "end": {"line": 31, "column": 0},
    "changed": [{"line": 30, "start_column": 8, "end_column": 9}]},
   {"id": 10, "kind": "leaf", "start": {"line": 31, "column": 0}, "end": {"line": 53, "column": 0}}]}
```

**Leaves** tile the file: read in order, their line ranges cover every line once.
They always start and end at column 0. A leaf with an `id` on both sides is
paired: the two have the same line count and their rows pair line for line. A
leaf on one side only has no counterpart, and the other side shows blank rows
against it. The row table is the walk over both sides' leaves, zipped by id.
A paired leaf whose counterpart lies behind the reading cursor is a move; the
frontend chooses how to show it.

`changed` holds the byte ranges inside a leaf that should be painted as changed:
a line that is entirely new carries one span covering it, a changed word inside
an otherwise matching line carries just that word. A line with no span in a leaf
that has spans is a changed line whose tokens all matched elsewhere. Blank
changed lines carry no span.

**Folds** are regions with `children`. Their line span is the hull of their
children, which tile it exactly; `start` and `end` keep the byte-precise range
the parser found, so the first line of a fold is its header and stays visible
when it collapses. A fold with an `id` on both sides is the same syntax node on
both sides; its contents may differ. Folds nest by containment. A syntactic
region that spans a single line is not a region: it hides nothing.

`tags` name what a region is (`body`, `import`, `test`, `unchanged`, or tags a
hook adds). `visibility` is how it starts out: `collapsed` and the `label` to
show while collapsed, a placeholder or pseudocode summary for a fold, or the
count for a context gap. Absent means open.

**Context** is expressed as leaves. The rows difftastic's hunks display,
which are the `-U` padding around every change plus the enclosing syntax
context such as the header of the function a change sits in, stay open.
Every other stretch of unchanged rows that is at least three lines long
collapses into a leaf tagged `unchanged` with a label such as
`"142 unchanged lines"`; shorter stretches stay open because a fold row
would save nothing. A file with no change is one collapsed leaf however
short. Folds that lie entirely inside a gap are not regions, so a gap is
split only by a fold that crosses its edge, and the grouping mutation merges
adjacent gaps again where both sides agree.

**Groups** come from the last built-in mutation: a run of two or more
sibling folds that start collapsed, such as several deleted or summarized
functions in a row, is wrapped in one new fold tagged `group` whose label
counts them (`"3 functions removed"`, `"3 functions summarized"`, or
`"3 folded regions"`). It starts collapsed; expanding it reveals each
child's own collapsed row. The wrapped children are untouched.

Ids are per file, dense, and assigned in lhs preorder then rhs preorder. They
mean nothing across files or runs.

## Computation and output

Discovery and rename detection finish before `start`; syntax matching is lazy.
A pool of `--jobs` workers pulls files from the iterator: each worker reads the
next file's sources under a lock, then diffs them while other workers pull
further files. The calling thread serializes, writes and flushes each record.
A bounded queue holds one ready record, so computation overlaps slow writes
without collecting the entire comparison. In-flight files are bounded by the
pool size, not their individual size.

Closing stdout stops production once the files in flight finish. Terminate the
process to cancel immediately. The CLI also retains its normal SIGPIPE behavior
on Unix.

Regular UTF-8 text files are supported. Binary and non-UTF-8 files, symlinks,
submodules and unmerged index entries produce per-file errors. Non-UTF-8 paths
fail discovery.

## Mutations and fold hooks

After projection, mutations set what starts collapsed and what its label says:
generated and test files in the manifest, deleted bodies, the middle of large
removed stretches, and summaries for new bodies from the built-in summarizer. [config.md](config.md) lists them and
their order. Their output is ordinary `visibility` on files and regions;
frontends need no knowledge of which mutation produced it.

A trusted hook can add its own summaries. It is a JSON-RPC 2.0 server over
HTTP that diffr starts once per invocation, calls on loopback, and runs after
the built-in summarizer:

```toml
[folds.hook]
command = ["uv", "run", "--script", "examples/hooks/summarize.py"]
tags = ["body"]            # optional; any listed tag qualifies. Omit to send every tagged fold.
min_lines = 12             # optional; defaults to folds.min_lines
timeout_ms = 5000          # optional; per call
startup_timeout_ms = 30000 # optional; time allowed to start listening
```

The command starts with the caller's environment plus `DIFFR_HOOK_PORT`, the
loopback port it must listen on, and `DIFFR_WORKSPACE`, the diffed repository's
root. It runs in the directory of the file that configured it, so relative
paths in `command` resolve against that file wherever it lives. Its stdout is
discarded because diffr's own stdout carries the stream; log to stderr. diffr
polls the port until the hook accepts connections, exits 2 before `start` if
the hook exits or misses `startup_timeout_ms`, and kills the hook when the
comparison ends.

Only new fold regions on the after side qualify: folds whose id has no
counterpart on the before side, of at least `min_lines` lines. Files with no
qualifying fold never reach the hook. Streaming is the only output mode that
runs mutations.

One call per file, method `summarize`, params by name. The worker diffing that
file blocks on the reply; other workers keep calling, so a hook must serve
requests concurrently rather than one at a time.

```jsonc
// diffr -> hook   POST / with a JSON-RPC 2.0 request
{"jsonrpc": "2.0", "id": 7, "method": "summarize", "params": {
  "path": "src/auth.py", "language": "Python", "src": "<after source>",
  "folds": [{"id": 3, "start": {"line": 40, "column": 0}, "end": {"line": 88, "column": 1},
             "tags": ["body"], "placeholder": "Body"}]}}
// hook -> diffr
{"jsonrpc": "2.0", "id": 7, "result": {"3": "def refresh_token(session):\n    ..."}}
```

A fold `id` is the region's id in that file's `rhs.regions`. The matching
region starts collapsed with the returned text as its label, behind a
`# pseudocode` comment line in the file's own syntax. Folds missing from the
result keep their placeholder. An error object, a timeout, an unknown fold id,
or an invalid response is a run-level failure: the stream ends with
`complete.aborted` set to `hook_failed` and diffr exits 2.

`examples/hooks/summarize.py` is a reference hook: an aiohttp server that hands
each request to jsonrpcserver and asks Gemini Flash, with thinking disabled,
for Python-style pseudocode. It needs `GOOGLE_API_KEY` and answers up to 16
files at once on one asyncio loop with a shared httpx client. `uv run --script`
installs its dependencies on first use. The built-in summarizer does the same
job without a subprocess. `tests/hooks/rpc_server.py` is a dependency-free hook
used by the tests.

## Fixture viewer

The static viewer under `examples/review/viewer` still reads the v1 stream;
its build script captures `--format ndjson-v1`:

```sh
cargo build --locked
python3 examples/review/viewer/build.py
python3 -m http.server 4176 --bind 127.0.0.1 --directory examples/review/viewer
python3 tests/streaming/check.py
```

Rebuild captures after backend changes. Static HTTP serves the example assets
only; it does not compute diffs.
