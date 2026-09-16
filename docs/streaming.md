# CLI diff streaming

```sh
diffr main HEAD --format ndjson
diffr --cached --format ndjson --order source,test -- src/
diffr --no-index --format ndjson -- before.rs after.rs
```

Spawn one process per comparison and consume stdout line by line. The comparison
arguments are the same as the [ordinary CLI](cli.md). There is no HTTP server.
`--format ndjson` is the stream described here. It requires a repository
comparison or `--no-index`; it rejects `--quiet` and metadata output flags.

The Rust types behind this document are `src/protocol/mod.rs`; the projection
from the internal diff is `src/protocol/project.rs`.

## Configuration and ordering

Each invocation reads the global configuration file (or `--config PATH`),
applies command-line flags, and compiles the result once; see
[config.md](config.md). Omitted keys
retain defaults; query strings replace whole values, and empty queries disable
that feature.

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
| `start` | `version: 3`, `lhs`, `rhs`, `files` | Lay out every file up front. |
| `file` | `file`, then `diff` or `error` | Render a result, or mark the file failed. |
| `complete` | `succeeded`, `failed` | Mark complete. |

`lhs` and `rhs` on `start` say what is being compared: `{"type":"revision","rev":"<oid>"}`,
`{"type":"index"}`, `{"type":"working_tree"}`, `{"type":"empty_tree"}` for an unborn
branch, or `{"type":"path","path":"…"}` for `--no-index`. Revisions resolve once.
Index content is pinned by blob id during discovery; working-tree files are read
as results are computed, not as an atomic snapshot.

`files` lists every selected file in priority order:

```jsonc
{"file": {"lhs": {"path": "src/a.rs", "oid": "3b18…", "mode": "100644"},
          "rhs": {"path": "src/a.rs", "oid": "9be2…", "mode": "100644"}},
 "status": "modified"}            // added | deleted | modified | renamed | copied | type_changed
```

`file` is git's delta: a deleted file has `lhs` only, an added file `rhs` only. The
path pair is the file's identity; each `file` record repeats it verbatim so the
record can be matched back to the manifest. Working-tree sides carry git's
all-zero oid. A `--no-index` comparison has empty `oid` and `mode`.

Results arrive in completion order, not manifest order, since files are diffed
concurrently (`--jobs`, default 16). `--jobs 1` restores priority order. At
completion, `succeeded + failed` equals the manifest length. EOF without
`complete` means the output was cut off.

### Errors

One shape everywhere: `{"code": "<snake_case>", "message": "<prose>"}`.

- On a `file` record, `error` replaces `diff` and the run continues. Codes:
  `binary`, `not_utf8`, `unsupported_file_type` (symlinks, submodules),
  `unmerged`, `read_failed`, `query_conflict` (two fold query patterns capture
  one syntax node with different fold ranges; the message names the line and
  both patterns), or `internal` for a failure diffr did not classify.
- Setup failures (bad revision, unreadable config, a query that does not
  compile) write to stderr and exit 2 before any record.

Exit status is 0 on success, 1 with `--exit-code` when there are changes, 2 when
any file failed.

## The diff

```jsonc
{"type": "text",
 "lhs": {"text": "…", "regions": [...]},
 "rhs": {"text": "…", "regions": [...]},
 "stats": {"textual": {"added": 4, "removed": 1}}}         // + "fallback": {code, message} on a line diff
```

A `binary` diff carries only `{"lhs": {"size": n}, "rhs": {"size": n}}`; either
side being binary makes the whole diff binary. Text sides are a pairing too: a
deleted file has `lhs` only.

`text` is the complete source.

`stats.textual` counts lines with any byte change.

`stats.fallback` is present when the AST match did not run: `unsupported_language`,
`too_large`, `too_complex`, `parse_error`. Its `message` is the engine's own
account of why, such as the size a file reached and the limit it exceeded. A
fallback diff is aligned by a line diff and its `changed` spans are word-level,
but the parse still stands: folds are present whenever the language parsed
(`too_complex`, `parse_error`). A line diff has no matcher, so its folds pair
with nothing. Only `unsupported_language` and `too_large` produce leaves
alone.

### Regions

Each side carries a tree of regions. A region is a line range on that side with
identities that never stand in for one another. `id` names the region: it is
unique within the file, across both sides.
`fold_state_id` says what the region opens and closes with: regions sharing it
open and close together, on the same side or across sides. Paired leaves and
matched folds share it across sides. Only a leaf has an
`alignment_id`, and it is row alignment: the same value on the other side marks
the leaf whose rows line up with this one, line for line. Consumers key the row
zip by leaf `alignment_id`, collapse state by `fold_state_id`, and anything
about the region itself by `id`.

```jsonc
{"id": 7, "fold_state_id": 7, "kind": "fold",
 "start": {"line": 18, "column": 0}, "end": {"line": 53, "column": 0},
 "tags": ["body"],
 "visibility": {"collapsed": false, "label": "Body"},
 "children": [
   {"id": 8, "fold_state_id": 8, "kind": "leaf", "alignment_id": 5, "start": {"line": 18, "column": 0}, "end": {"line": 30, "column": 0}},
   {"id": 9, "fold_state_id": 9, "kind": "leaf", "alignment_id": 6, "start": {"line": 30, "column": 0}, "end": {"line": 31, "column": 0},
    "changed": [{"line": 30, "start_column": 8, "end_column": 9}]},
   {"id": 10, "fold_state_id": 10, "kind": "leaf", "alignment_id": 7, "start": {"line": 31, "column": 0}, "end": {"line": 53, "column": 0}}]}
```

**Leaves** tile the file: read in order, their line ranges cover every line once.
They always start and end at column 0. A leaf whose `alignment_id` a leaf on the
other side also carries is paired: the two have the same line count and their
rows pair line for line. A
leaf on one side only has no counterpart, and the other side shows blank rows
against it. The row table is the walk over both sides' leaves, zipped by
`alignment_id`. Paired leaves come in the same order on both sides.

`changed` holds the byte ranges inside a leaf that should be painted as changed:
a line that is entirely new carries one span covering it, a changed word inside
an otherwise matching line carries just that word. A line with no span in a leaf
that has spans is a changed line whose tokens all matched elsewhere. Blank
changed lines carry no span.

**Folds** are regions with `children`. Their `start` and `end` are the hull of
their children, which tile it exactly, so a fold's range is whole lines: exactly
the lines collapsing it hides. A fold covers only the lines it holds whole. A
body fold starts on the line after the `{` or `:` that opens it, because the
header line holds code the fold does not cover; that line is the last line of
the leaf before the fold. The same at the end: a fold stops above the line its
`}` sits on, whether the brace is in column 0 or not. A fold whose node begins
its line, such as a comment block or an import, starts on that line. Regions form
a strict tree: every child lies inside its parent's range and siblings never
overlap; where the parser still hands over two folds that cross on one line,
the earlier one gives that line to the later. Two queries that fold the same lines,
from one plugin or two, make one fold carrying both their tags: a fold is a
region of the file, and it belongs to the innermost syntax node that covers
that region, whose pairing it takes. Two folds that share `fold_state_id`
across sides are matched: the syntax nodes they were built on are the same node
on both sides, as the matcher paired them, whose contents may differ. A fold is
paired exactly when a region on the other side shares its `fold_state_id`. A
syntactic region that
spans a single line is not a region: it hides nothing.

`tags` name what a region is. A leaf carries none. A fold carries
the tags the fold queries set on it (`body`, `import`, `test`). Tags describe
syntax; how a region starts out is `visibility`, and frontends need no
knowledge of tags to render it. `visibility` is `collapsed` and the `label` to
show while collapsed: a placeholder named after the fold's first tag. Nothing
starts collapsed yet. Absent means open.

Ids are per file. The projection numbers the regions as it builds them, in lhs
preorder then rhs preorder, from two counters: `id` starts at 1 and
`alignment_id` at 0. Every region takes the next `id`, and every leaf takes
the next `alignment_id` unless
its counterpart on the other side was numbered first, whose `alignment_id` it
takes instead. A region's `fold_state_id` is its own `id`, except that a paired
leaf or matched fold numbered second takes its counterpart's `fold_state_id`.
No region has `id` 0: it names the file itself. Ids mean nothing across files
or runs.

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
