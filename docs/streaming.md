# Local diff streaming

```sh
difft serve --repo /path/to/workspace --listen 127.0.0.1:4176
```

One server binds to one workspace. The command remains `difft` until the separate
rename change. Only loopback listening is supported.

## Configuration and ordering

The server reads repository-root `diffr.toml` once at startup. `--config PATH`
selects another file instead of the repository file. A missing repository file
uses defaults; a missing explicit file or invalid configuration is an error.

Supplied keys override bundled defaults. Query strings replace whole
values. Omitted keys retain defaults; an empty query disables that feature.

Classifications use the current workspace's Git attributes, not attributes from
the requested revisions. Git resolves normal nested/global attribute precedence:

```gitattributes
*          diffr-classify=source
tests/**   diffr-classify=test
docs/**    diffr-classify=docs
**/*.lock  diffr-classify=generated
```

Only string-valued attributes assign a class. Set/unset/unspecified attributes
leave the file unclassified. Unlisted and unclassified files sort last; ties use
path order. Renames classify by new path, deletions by old path.

Discovery copies classifications into descriptors, fixing ordering for the request.
Discovery and rename detection finish before the first event. Rename detection can
read file contents; syntax matching starts afterward, one file at a time.

## Request

`POST /diff` with `Content-Type: application/json`:

```json
{"base":"main","head":"HEAD","files":{"order":["source","test"],"paths":["src/lib.rs"]}}
```

Only `base` and `head` are required. They resolve to pinned commit IDs. This is
direct base-to-head comparison, with no implicit merge-base calculation.

- `files` groups client selection and ordering. It may be omitted.
- Omitted `files.order` or `[]` means path order only; no server default exists.
- Omitted `files.paths` selects all changed files. Supplied paths are exact, not globs;
  either side of a rename can select it. Explicit unchanged files are returned too.
  Missing paths fail before streaming.

## Response contract

HTTP 200 uses `application/x-ndjson`. Decode full newline-delimited JSON records:
network chunks can split a record or contain several.

| Event | Fields | Client action |
| --- | --- | --- |
| `start` | `version: 1`, resolved `base`, `head`, `total` | Initialize progress. |
| `file` | `file`, `diff` | Render immediately. |
| `file_error` | `file`, `message` | Show the failure and continue reading. |
| `complete` | `succeeded`, `failed` | Mark complete, including partial failures. |
| `error` | `message` | Terminal worker failure; mark incomplete. |

The file descriptor has nullable `old_path`, `new_path`, `class`, and a `status`
of added/deleted/modified/renamed/type_changed/unchanged. Unchanged applies only to
explicit paths. `diff` is the existing domain JSON: sources, token correspondence,
folds, and syntax-context hunks. Display alignment is computed by the client;
layout is never included in the response.

Invalid refs/paths return HTTP 400 with `{"error":"..."}` before streaming.
JSON extraction failures use the framework's non-200 error response. Preparation
worker failures return HTTP 500. After streaming starts, failures use events.

EOF without `complete` means incomplete, regardless of HTTP 200.
At completion, `succeeded + failed == total`, and each selected file has exactly
one file or file_error event.

Cancel with AbortController. Dropping the response stops scheduling subsequent
files. An already running file computation may finish after disconnection.
There is no parallel file scheduler or full-result buffer.

Current scope is UTF-8 regular text files. Binary/non-UTF-8 contents, symlinks and
submodules produce per-file errors; later files continue. Non-UTF-8 paths fail
discovery.

## Example and checks

The [example client](../examples/review/viewer/stream.mjs) renders file events and
cancels old requests on navigation. Git baseline text is precomputed; domain diffs
are computed live.

```sh
cargo build --locked
python3 examples/review/viewer/build.py
target/debug/difft serve --repo examples/review/viewer/data/workspace \
  --web-root examples/review/viewer --listen 127.0.0.1:4176
python3 tests/streaming/check.py
```
