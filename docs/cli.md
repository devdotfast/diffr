# Git-style CLI

```sh
diffr                         # index -> working tree
diffr --cached                # HEAD -> index (empty tree on an unborn branch)
diffr HEAD                    # HEAD -> working tree
diffr main HEAD               # two revisions
diffr main...HEAD             # merge-base(main, HEAD) -> HEAD
diffr main..HEAD              # main -> HEAD
diffr --merge-base main       # merge-base(main, HEAD) -> working tree
diffr HEAD -- src/ '*.rs'      # repository selection, with pathspecs
diffr --no-index before.rs after.rs
```

`--repo DIR` selects a repository; otherwise discovery starts in the current
directory. Paths are relative to that directory. Use `--` to separate paths
from revisions. `-R` reverses the comparison. `--staged` aliases `--cached`.

`--name-only`, `--name-status`, `--stat`, `--shortstat`, and `--numstat` use
libgit2 without compiling syntax configuration or matching syntax. Counts are
textual. Name status uses A/D/M/R/T/U; rename similarity scores are not exposed.
`-z` supports name-only and name-status output for unambiguous path delimiters.
Text output does not implement Git's path quoting or compact rename formatting.
`-M` enables rename detection (the default); `--no-renames` disables it.

`--quiet` suppresses output and exits 1 for changes. `--exit-code` also exits 1
for changes; ordinary output exits 0. Errors exit 2. `--no-index` supports two
files, implies change exit status, and does not yet support metadata options.

A comparison opens the terminal UI (`tui/`), which needs a terminal on stdin
and stdout; without one diffr exits 2. `--format ndjson` instead writes the
event stream described in [streaming.md](streaming.md), diffing `--jobs N`
files at once (default 16) and emitting each as it finishes; the terminal UI
reads the same stream. `--format patch` writes a plain-text patch instead; see
[Patch output](#patch-output).
`-U N` sets the unchanged lines kept around each change, overriding
`plugins.bundled.context.lines` (default 3). Matching limits and `--ignore-comments`
remain configurable; `--width` sets the columns of `--stat`. See `--help`.

Configuration comes from the global file, command-line flags and git
attributes; `--config PATH` replaces the global file. `diffr config` opens the
settings screen, and `diffr config schema`, `show` and `set` are the commands
frontends use. See [config.md](config.md).

This is a subset of git diff, not full flag parity: unsupported options and Git
magic pathspecs fail explicitly. Untracked files are excluded as in git diff.
`diffr debug` keeps difftastic's syntax dumps (`--dump-ts`, `--dump-syntax`,
`--dump-syntax-dot`) and `--list-languages` for diagnostics. The stdout event
contract is in [streaming.md](streaming.md).

## Patch output

```sh
diffr main...HEAD --format patch -- src/
diffr --cached --format patch --no-folds
```

`--format patch` prints the comparison as a Git-style patch for a person or an
agent to read. It is built from the same shaped records as `--format ndjson`
and the terminal UI, not from a separate line diff, so it keeps diffr's
structure: a region the plugins start collapsed prints as one marker line
instead of its lines.

```text
diff --git a/src/lib.rs b/src/lib.rs
 1  1  struct Parser;
 2  2
 3  3  impl Parser {
@@ … 13 unchanged lines · impl Parser › fn keep(&self) @@
17 17      }
18 18
19 19      fn parse(&self) {
20    -        self.old();
   20 +        self.new();
21 21          self.same();
22 22      }
23 23  }
diff --git a/tests/it.rs b/tests/it.rs
@@ … file folded: Test file · hidden by default, +1 −1 @@
```

With the summarizer on, a new function's body is its summary:

```text
diff --git a/a.rs b/a.rs
@@ … 2 lines of documentation · fn total(a: u32, b: u32, c: u32) -> u32 @@
  3 +fn total(a: u32, b: u32, c: u32) -> u32 {
@@ … 4 lines, summarized · fn total(a: u32, b: u32, c: u32) -> u32 @@
    ~    x, y, z = a, b, c
    ~    return x + y + z
  8 +}
```

- Each file starts with `diff --git a/<old> b/<new>` and, as Git prints them,
  `new file mode`, `deleted file mode`, `old mode`/`new mode` and
  `rename from`/`rename to` (or `copy`). `index`, `---` and `+++` are left out.
- Every code line is `<base> <head> <marker><text>`: 1-based line numbers
  right-aligned to the file's widest, blank on the side the line is not on,
  then ` `, `-` or `+`. This is the gutter of Review's numbered patches, so
  head numbers can be cited directly. Removed lines of a changed block print
  before its added lines.
- A collapsed region is `@@ … <label> · <scopes> @@`: the plugin's label
  (`12 unchanged lines`, `8 lines removed`, `test body`,
  `3 collapsed regions · 42 lines`), or a line count when it has none, then up
  to three enclosing scopes, innermost last. Consecutive context gaps add up
  into one marker under the scopes they share. Open regions print their lines.
- A summarized fold is `@@ … N lines, summarized · … @@` followed by the
  summary, one `~` line each, indented like the code it stands for.
- A file the plugins hide (generated, vendored, test or deleted files, by
  default) is its header and one marker:
  `@@ … file folded: Test file · hidden by default, +41 −0 @@`.
- A file whose structural diff fell back to a line diff says why under its
  header: `Line diff (too_complex): …`. Files without a grammar are always
  line diffs and say nothing. A binary file prints
  `Binary files a/<old> and b/<new> differ`; a file that failed prints
  `Error (<code>): <message>`. A file with no changed line prints only its
  header.

Files are diffed on `--jobs` workers and printed in file order (`--order`,
else path order) as soon as every file before them is done, so the output is
the same for any `-j`. `-U`, `-R`, pathspecs, revisions and the engine limits
work as for the stream. The output is plain text; there is no color.

Deferred annotations, such as the summarizer's pseudocode, are waited for:
each file prints once with its summaries, as the default `ndjson` stream does,
because a text patch cannot be amended after it is written. `--no-annotations`
skips them, leaving a summarized fold as `N lines, not summarized`.
`--no-folds` prints every region and hidden file in full, with no markers; it
skips annotations, since no fold is left to label. `--syntax` and
`--stream-annotations` apply only to `ndjson`.
