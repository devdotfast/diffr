# CLI diff streaming

```sh
diffr main HEAD --format ndjson
diffr --cached --format ndjson --order source,test -- src/
```

Spawn one process per comparison and consume stdout line by line. The comparison
arguments are the same as the [ordinary CLI](cli.md). There is no HTTP server.
`--format json` remains the bare domain-object output; `ndjson` adds file metadata,
progress, recoverable errors and a completion record. NDJSON currently requires a
repository comparison; it rejects `--no-index`, `--quiet` and metadata output flags.

## Configuration and ordering

Each invocation loads repository-root `diffr.toml` and compiles it once.
`--config PATH` selects another file instead. Omitted keys retain bundled defaults;
query strings replace whole values, and empty queries disable that feature.

File classes come from the current workspace's Git attributes:

```gitattributes
*          diffr-classify=source
tests/**   diffr-classify=test
docs/**    diffr-classify=docs
**/*.lock  diffr-classify=generated
```

`--order source,test,docs` prioritizes those classes. It can also be repeated.
Unlisted and unclassified files follow, with path order breaking ties. Without
`--order`, use path order. Renames classify by new path; deletions by old path.
Only string attributes assign classes. Normal Git attribute precedence applies.

Paths after `--` accept libgit2 directory prefixes and wildcard patterns, not Git
magic pathspecs. Only changed files are selected. An unmatched path yields an empty
stream. Filtering precedes rename detection, so selecting one side of a rename
can appear as an addition or deletion. `--no-renames` skips rename detection.

## Output contract

Stdout contains only UTF-8 newline-delimited JSON records. Read complete lines;
pipe reads can split records or contain several.

| Event | Fields | Consumer action |
| --- | --- | --- |
| `start` | `version: 1`, `before`, `after`, `total`, `files` | Lay out every file up front. |
| `file` | `file`, `diff`, optional `hook_error` | Render a result. |
| `file_error` | `file`, `message` | Report failure and keep reading. |
| `complete` | `succeeded`, `failed` | Mark complete, including partial failures. |

`before` and `after` identify the operands: `{"kind":"revision","ref":"<oid>"}`,
`{"kind":"index"}`, `{"kind":"working_tree"}`, or `{"kind":"empty_tree"}`.
Revision refs resolve once. Index source is pinned by blob ID during discovery.
Worktree files are read as results are computed, not as an atomic snapshot.

The file descriptor contains nullable `old_path`, `new_path`, `class`, and `status`
(added/deleted/modified/renamed/type_changed/conflicted). `diff` is the existing
domain JSON: complete sources, token correspondence, folds and context hunks.
There is no display layout in the response.

`files` lists every selected file descriptor in priority order. File results
arrive in completion order, not manifest order, since files are diffed
concurrently (`--jobs`, default 16). Match results to the manifest by identity.

At completion, `succeeded + failed == total`; every selected file has one result
or file error. EOF without `complete` means interrupted/incomplete output.
Setup failures write to stderr and exit 2 before producing any records.
Per-file failures emit `file_error`, allow subsequent results, and finish with
`complete` and exit 2. Success exits 0, or 1 with `--exit-code` if changes exist.
Unexpected computation or output failures can terminate without `complete`.

Regular UTF-8 text files are supported. Binary/non-UTF-8 files, symlinks,
submodules and unmerged index entries produce per-file errors. Non-UTF-8 paths
fail discovery.

## Computation and output

Discovery and rename detection finish before `start`; syntax matching is lazy.
A pool of `--jobs` workers pulls files from the iterator: each worker reads the
next file's sources under a lock, then diffs them while other workers pull
further files. The calling thread serializes, writes and flushes each event.
A bounded queue holds one ready event, so computation overlaps slow writes
without collecting the entire comparison. In-flight files are bounded by the
pool size, not their individual size. `--jobs 1` restores priority order.

Closing stdout stops production once the files in flight finish. Terminate the
process to cancel immediately. The CLI also retains its normal SIGPIPE behavior
on Unix.

## Fold hooks

A trusted hook subprocess can replace fold placeholders with richer text, such
as pseudocode, before each `file` event is emitted:

```toml
[folds.hook]
command = ["uv", "run", "--script", "examples/hooks/summarize.py"]
tags = ["body"]     # optional; any listed tag qualifies. Omit to send every fold.
min_lines = 12      # optional; default 0
timeout_ms = 5000   # optional; per request
```

The command starts once per invocation with the caller's environment. It runs
in the directory containing the config file, so relative paths in `command`
resolve against the config wherever it lives, including one given by
`--config` outside the repository. `DIFFR_WORKSPACE` carries the diffed
repository's root. Only novel folds on the after side qualify: bodies that
exist in the after source with no counterpart in the before source. Files with
no qualifying fold never reach the hook. Streaming is the only output mode that
runs hooks; the terminal frontend streams, so it does too.

Requests are one JSON line per file on the hook's stdin, and replies are one JSON
line per request on its stdout, matched by `id` and accepted in any order. The
worker diffing a file blocks on that file's reply; other workers keep going, so
a hook must answer requests concurrently rather than one at a time.

```jsonc
// diffr -> hook
{"id": 7, "path": "src/auth.py", "language": "Python", "src": "<after source>",
 "folds": [{"id": 0, "range": {"start": {"line": 40, "byte_column": 0}, "end": {"line": 88, "byte_column": 1}},
            "tags": ["body"], "placeholder": "Body"}]}
// hook -> diffr
{"id": 7, "texts": {"0": "def refresh_token(session):\n    ..."}}
{"id": 8, "error": "rate limited"}
```

`language` is null for plain text. A fold `id` indexes `rhs_folds` in that file's
`diff`; the matching fold gains a non-null `summary` while `placeholder` is
unchanged. Folds missing from `texts` keep a null `summary`. An `error` reply, a
timeout, an unknown fold id, a malformed line, or hook exit leaves every summary
in that file null and adds `hook_error` to its `file` event. A malformed line or
exit also fails every later request, since ids can no longer be trusted. Hook
stderr passes through to diffr's stderr. Failing to start the command exits 2
before `start`.

`examples/hooks/summarize.py` is a reference hook that asks Gemini 3.8 Flash,
with thinking disabled, for Python-style pseudocode. It needs `GOOGLE_API_KEY`
and answers up to 16 files at once.

## Fixture viewer

The viewer reads captured CLI streams from static files:

```sh
cargo build --locked
python3 examples/review/viewer/build.py
python3 -m http.server 4176 --bind 127.0.0.1 --directory examples/review/viewer
python3 tests/streaming/check.py
```

Rebuild captures after backend changes. Static HTTP serves the example assets
only; it does not compute diffs.
