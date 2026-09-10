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
| `start` | `version: 1`, `before`, `after`, `total` | Initialize progress. |
| `file` | `file`, `diff` | Render a result. |
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
A producer thread consumes the file iterator. The calling thread serializes,
writes and flushes each event. A bounded queue holds one ready event, allowing
computation to overlap slow writes without collecting the entire comparison.
When the queue is full, the producer waits. This bounds the number of in-flight
files, not their individual size.

Closing stdout stops production when its next send fails; an already running file
may finish. Terminate the process to cancel immediately. The CLI also retains its
normal SIGPIPE behavior on Unix.

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
