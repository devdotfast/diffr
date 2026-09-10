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

Structural output uses the existing terminal renderer. `--format json` emits
one domain object per line; `--format snapshot` is the fixture text adapter.
`-U N` selects ordinary context padding. Matching limits, `--ignore-comments`,
color, width and inline/split display remain configurable; see `--help`.

This is a subset of git diff, not full flag parity: unsupported options and Git
magic pathspecs fail explicitly. Untracked files are excluded as in git diff.
The old file/external-diff/debug parser is available under `debug` for diagnostic
use; it is not the default argument syntax. `review` is replaced by the normal
CLI's `--format` option. The HTTP contract is in [streaming.md](streaming.md).
