% DIFFR(1)
% diffr contributors

# NAME

diffr - structural Git comparisons and local diff streaming

# SYNOPSIS

**diffr** [OPTIONS] [REVISION [REVISION]] [**--** PATHSPEC...]

**diffr** **--no-index** BEFORE AFTER

**diffr server** **--repo** DIRECTORY [**--listen** ADDRESS]

# DESCRIPTION

Without revisions, compare index to working tree. With --cached, compare HEAD to
index. With one revision, compare that revision to working tree. With two,
compare their trees. A...B compares merge-base(A,B) to B; A..B compares A to B.

# OPTIONS

**--repo** DIRECTORY
: Discover the repository from DIRECTORY (default current directory).

**--cached**, **--staged**
: Compare a revision (default HEAD) to the index.

**-R**
: Reverse before and after.

**--name-only**, **--name-status**, **--stat**, **--numstat**, **--shortstat**
: Print metadata without syntax matching. Stat counts are textual.

**--quiet**, **--exit-code**
: Exit 1 for changes. Quiet also suppresses output. Errors exit 2.

**--format** text|json|snapshot
: Terminal rendering, domain JSON records, or fixture text.

**-U** N
: Select ordinary context padding (default 3).

**--config** FILE
: Read explicit TOML configuration instead of repository diffr.toml.

**--help**
: List the supported options. Unsupported Git flags are rejected.

# SERVER

The server accepts POST /diff with before and after operands and emits NDJSON.
See docs/streaming.md in the source distribution for the request and event schema.
