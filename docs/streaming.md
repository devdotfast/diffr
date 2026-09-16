# CLI diff streaming

```sh
diffr main HEAD --format ndjson
diffr --cached --format ndjson --order test,docs -- src/
diffr --no-index --format ndjson -- before.rs after.rs
```

Spawn one process per comparison and consume stdout line by line. The comparison
arguments are the same as the [ordinary CLI](cli.md). There is no HTTP server.
`--format ndjson` is the stream described here. It requires a repository
comparison or `--no-index`; it rejects `--quiet` and metadata output flags.

The Rust types behind this document are `src/protocol/mod.rs`; the projection
from the internal diff is `src/protocol/project.rs`, the plugin host that
shapes it is `src/plugin/`, and the bundled plugins are under `plugins/`.

## Configuration and ordering

Each invocation reads the global configuration file (or `--config PATH`),
applies command-line flags, and compiles the result once; see
[config.md](config.md). Omitted keys
retain defaults. The fold query for each language is assembled from the query
files of the enabled plugins; see [plugins](#plugins).

File tags come from bundled GitHub Linguist rules and diffr's test path rules,
then git attributes (`linguist-generated`, `linguist-vendored`,
`linguist-documentation`, `diffr-tags`); [config.md](config.md#file-tags) has
the precedence. Renames are tagged by new path; deletions by old path.

`--order test,docs` puts files carrying those tags first, ranked by the
earliest listed tag a file carries. It can also be repeated. Files with none of
the listed tags follow, with path order breaking ties. Without `--order`, use
path order.

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
| `file` | `file`, optional `visibility`, then `diff` or `error` | Render a result, or mark the file failed. |
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
 "tags": ["generated", "vendored"]} // sorted; absent when none
```

`file` is git's delta: a deleted file has `lhs` only, an added file `rhs` only. The
path pair is the file's identity; each `file` record repeats it verbatim so the
record can be matched back to the manifest. Tags are known before `start`:
reading the start of a file for the header checks happens during discovery,
and if that read fails the file's record carries a `read_failed` error. Whether
a file starts hidden is not known yet: the plugins decide that after diffing, and
the `file` record carries it. Working-tree sides carry git's
all-zero oid. A `--no-index` comparison has empty `oid` and `mode`.

A `file` record's `visibility` is how the file starts out, set by the plugins:
`{"collapsed": true, "label": "Generated file · hidden by default"}` for a
hidden file, absent for an open one. A file whose record is an `error` has no
`visibility`.

Results arrive in completion order, not manifest order, since files are diffed
concurrently (`--jobs`, default 16). `--jobs 1` restores priority order. At
completion, `succeeded + failed` equals the manifest length unless `aborted` is
present. EOF without `complete` means the output was cut off.

### Errors

One shape everywhere: `{"code": "<snake_case>", "message": "<prose>"}`.

- On a `file` record, `error` replaces `diff` and the run continues. Codes:
  `binary`, `not_utf8`, `unsupported_file_type` (symlinks, submodules),
  `unmerged`, `read_failed`, `query_conflict` (two query files capture one
  syntax node with different fold ranges; the message names the line and both
  files), or `internal` for a failure diffr did not classify.
- On `complete`, `aborted` reports a run-level failure in a plugin:
  `mutation_failed` when a plugin's `mutate` returns an error (the
  summarizer's model call failing after its retries, say) or when a plugin
  asks for a move that cannot be carried out. The message names the
  plugin. diffr
  stops pulling files, lets the ones in flight finish, and exits 2. Every
  `file` record already written stays valid; the file that failed has no
  record.
- Setup failures (bad revision, unreadable config, a query file that does not
  compile, a malformed `diffr-tags` attribute, a plugin option that does not
  match its schema, a plugin that cannot be made, such as a summarizer
  without an API key, or a `classify` that fails or returns a malformed tag)
  write to stderr
  and exit 2 before any record.

Exit status is 0 on success, 1 with `--exit-code` when there are changes, 2 when
any file failed or the run aborted.

## The diff

```jsonc
{"type": "text",
 "lhs": {"text": "…", "regions": [...]},
 "rhs": {"text": "…", "regions": [...]},
 "stats": {"textual": {"added": 4, "removed": 1},
           "visible": {"added": 2, "removed": 1}}}          // + "fallback": {code, message} on a line diff
```

A `binary` diff carries only `{"lhs": {"size": n}, "rhs": {"size": n}}`; either
side being binary makes the whole diff binary. Text sides are a pairing too: a
deleted file has `lhs` only.

`text` is the complete source.

`stats.textual` counts lines with any byte change. `stats.visible` counts the
changed lines still on screen under the default visibility: a line that carries
a `changed` span, or any line of a leaf that exists on one side only, unless it
sits inside a region that starts collapsed. It is computed after the plugins
run, so configuration changes it, and with nothing collapsed it matches
`textual` up to the blank lines of a paired changed run. A frontend that lets the
reader fold and unfold recomputes the same rule locally; the wire value is the
starting point.

`stats.fallback` is present when the AST match did not run: `unsupported_language`,
`too_large`, `too_complex`, `parse_error`, or `generated` for a file tagged
`generated`, which is always diffed by line without parsing. Its `message` is
the engine's own account of why, such as the size a file reached and the limit
it exceeded. A fallback diff is aligned by a line diff and its `changed` spans
are word-level, but the parse still stands: folds are present whenever the
language parsed (`too_complex`, `parse_error`). A line diff has no matcher, so
its folds pair with nothing. Only `unsupported_language`, `too_large` and
`generated` produce leaves alone.

### Regions

Each side carries a tree of regions. A region is a line range on that side with
identities that never stand in for one another. `id` names the region: it is
unique within the file, across both sides, and it is what plugins address.
`fold_state_id` says what the region opens and closes with: regions sharing it
open and close together, on the same side or across sides. Paired leaves and
matched folds share it across sides, and a plugin that links regions gives
several regions one
`fold_state_id` while each keeps its own `id`. Only a leaf has an
`alignment_id`, and it is row alignment: the same value on the other side marks
the leaf whose rows line up with this one, line for line. Consumers key the row
zip by leaf `alignment_id`, collapse state by `fold_state_id`, and anything
about the region itself by `id`.

```jsonc
{"id": 7, "fold_state_id": 7, "kind": "fold",
 "start": {"line": 18, "column": 0}, "end": {"line": 53, "column": 0},
 "tags": ["context:scope"],
 "visibility": {"collapsed": false, "label": ""},
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
the tags the fold queries set on it, each written `<plugin>:<name>` after the
plugin whose query set it (`context:scope`); a fold only some query made
exist without meaning carries none. Tags describe syntax, and are the only
syntax a plugin sees; how a region starts out is `visibility`,
and frontends need no knowledge of tags to render it. `visibility` is
`collapsed` and the `label` to show while collapsed: the count for a
collapsed stretch of unchanged lines, say.
A label is the plugin's that collapsed the fold, so a fold no plugin
collapsed has none, and the empty label shows the source. Absent means open.

**Context** comes from the `context` plugin; the projection itself hides
nothing. A line stays open when it is within `plugins.context.lines`
(`-U`, default 3) of a changed line, when it is paired with such a line, or
when it opens or closes a scope, such as the function a change
sits in, that contains a change. Scopes are folds the plugin's own queries
tag `context:scope`, on the construct's own node: a scope runs from the line
its signature starts on to the line that closes it, so keeping its first and
last line always shows where it opens and where it ends, however many lines
the signature takes. The body fold inside it covers the body alone and starts
a line later, so the two never share a range. Every other stretch of unchanged paired lines that
is at least three lines long collapses with a label such as
`"142 unchanged lines"`; shorter stretches stay open because a row would save
nothing. A file with no change collapses whole however short.


**Groups** come from the `group` plugin: a run of two or more sibling
regions that start collapsed, whichever plugin collapsed them, is wrapped in
one new fold, with no tags. Between two collapsed regions of a run there may
be open leaves spanning at most two lines in all, such as a blank line and the
next function's header. The label counts the collapsed regions and the lines
the group spans: `"3 collapsed regions · 42 lines"`. It starts collapsed;
expanding it reveals each child's own row. The wrapped children are untouched.
When the other side pairs a region of the run, the run is grouped only if the
other side holds a run that matches it region for region, and then both are
grouped with one fold state.

Region edges cut a stretch. The part of it among one list of siblings
collapses when it is at least three lines long or is the whole stretch, so a
sliver at a fold edge stays open. A part inside one leaf is that leaf's lines,
cut out. A part that spans several siblings, folds among them, collapses under
one new fold with no tags, labelled with the part's line count. A fold collapses only when it
lies wholly inside the stretch, and so does every region on the other side in
its fold state, so a fold whose matched counterpart holds changes stays open.
Both folds of a matched pair take the label. The group is made only when the
two sides' siblings match one for one (leaves sharing an `alignment_id`, or
folds sharing a fold state), and then on both sides, the two group folds
sharing a fold state;
otherwise the regions collapse one by one. A stretch that crosses the end of a fold collapses on
each side of that edge.

Ids are per file. The projection numbers the regions as it builds them, in lhs
preorder then rhs preorder, from two counters: `id` starts at 1 and
`alignment_id` at 0. Every region takes the next `id`, and every leaf takes
the next `alignment_id` unless
its counterpart on the other side was numbered first, whose `alignment_id` it
takes instead. A region's `fold_state_id` is its own `id`, except that a paired
leaf or matched fold numbered second takes its counterpart's `fold_state_id`.
Plugins take `id`s and `alignment_id`s above every one already in the file.
No region has `id` 0: plugins use it to name the file itself. Ids mean nothing
across files or runs.

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

## Plugins

Each file goes through one pipeline before its record is written:

1. Git lists the file and its tags ([config.md](config.md#file-tags)), and
   each enabled plugin's `classify` adds tags of its own, before the
   `start` header is written.
2. The diff runs. Its fold query for the file's language is assembled from
   the query files of every enabled plugin ([config.md](config.md#queries)).
3. The projection builds the region trees above and pairs them. Nothing
   starts collapsed yet.
4. The enabled plugins run in `plugins.order` on the finished trees, starting
   with `context`.
5. `stats.visible` is recounted, and the record is written.

Queries decide which regions exist; plugins decide how they start out. No
plugin matches one side to the other: pairing is the projection's alone. A
plugin counts a leaf as paired when the other side holds its `alignment_id`,
and a fold as paired when a region on the other side shares its
`fold_state_id`. Only the other side counts, since a link shares a fold state
within one side too. Whether a fold is new, deleted or unpaired is a
different question, and the leaves answer it: a fold is new when no line
inside it pairs with the other side, whatever the matcher made of the two
nodes, and however the file was diffed.

A plugin's `mutate` step reads the file (its paths, status and tags) and both
sides, as the records of `wit/plugin.wit`, and returns moves. It never edits a
tree itself; one applier, the plugin SDK's, carries the moves out, in order,
and the next plugin sees the result. A move names a
region by its `id`, which is on one side only, or the file by `0`, the root
every region hangs from:

- **Cut** a leaf at a line offset, relative to its first line and strictly
  inside it. The leaf paired with it (the one on the other side with its
  `alignment_id`) is cut at the same offset. The first piece on each side
  keeps its leaf's ids; the second takes a fresh `id` of its own and a fresh
  `alignment_id` shared with its counterpart, so the rows still pair, and the
  lhs piece's `id` as its `fold_state_id`. A fold, or the file, cannot be
  cut.
- **Join folds**: wrap consecutive sibling regions in a new fold, open,
  unlabelled and without tags. Each side wraps the listed regions it holds,
  so one move can list a run and the run matched with it on the other side;
  the two new folds take their own `id`s and share one `fold_state_id`.
- **Link fold state**: every region in the listed regions' fold states, on
  both sides, takes the first region's `fold_state_id` and whether it starts
  collapsed, so they open and close together. Today the bundled link is a
  docstring: a plugin that collapses a function body (`deleted-bodies`,
  `test-bodies`, `summarize`) links the body's docstring, a fold tagged
  `deleted-bodies:docstring`, `test-bodies:docstring` or (when the summarizer
  is on) `summarize:docstring`, to it. The docstring
  then carries the body's `fold_state_id` and starts collapsed, with an empty
  label. A docstring whose body no plugin collapses is not linked.
- **Set collapsed** on a region: every region sharing its `fold_state_id`
  starts collapsed, or open. On the file, whether the file starts hidden: the
  record's `visibility`.
- **Set label** on one region, or on the file: the label shown while it is
  collapsed.
- **Set tags** on one region, replacing them. The file's `tags` are its
  manifest entry's, known before any diff runs, so the file's cannot be set.

Collapsing a region behind a label is a set collapsed and a set label (after
a cut, for some lines of a leaf); grouping is a join, a set collapsed and a
set label on each new fold; hiding a file is a set collapsed and a set label
on the file. When the bundled plugins collapse a region, the regions that
start collapsed with it and were open lose their labels, and the
leaf paired with a collapsed leaf takes its label.

New `id`s are larger than every `id` already in the file, and new
`alignment_id`s larger than every leaf's. A move that cannot be carried out
(an unknown region, an offset outside a leaf, a cut on a fold, a join of
regions that are not consecutive siblings, or of a run one side holds only
one region of) is an error: the run aborts with `mutation_failed`.

The bundled plugins, in their default order, each a folder under `plugins/`:

| Plugin | What starts out differently |
| --- | --- |
| `context` | Unchanged lines far from any change and outside its enclosing headers collapse: `"142 unchanged lines"`. |
| `hide-files` | Files with a listed tag (`generated`, `vendored`, `test`), and deleted files, are hidden: `"Generated file · hidden by default"`. |
| `deleted-bodies` | A function body of at least 12 lines with nothing paired under it collapses: `"20 lines removed"`. Its docstring is linked to it. |
| `test-bodies` | Test function bodies (`"test body"`) and Rust `#[cfg(test)]` modules (`"test module"`) collapse on both sides. A test body's docstring is linked to it. |
| `removed-runs` | A one-sided leaf of at least 5 lines in removed (not rewritten) code keeps its first and last line open and collapses the rest: `"8 lines removed"`. |
| `summarize` | Off by default. A new function body of at least 20 lines collapses behind pseudocode from the configured model. The body is chosen as new before its docstring is linked to it; the pseudocode may quote the docstring. On without an API key, diffr stops before the stream starts. |
| `group` | Two or more adjacent collapsed regions are wrapped in one collapsed fold: `"3 collapsed regions · 42 lines"`. An open fold showing only collapsed regions and at most two open lines, such as a function's scope around its collapsed body and closing brace, counts as collapsed. |

A binary diff has no regions, so only `hide-files` can change how it starts
out. Streaming is the only output mode that runs plugins.

