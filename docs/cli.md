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
reads the same stream. `ndjson` is the only format.
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
