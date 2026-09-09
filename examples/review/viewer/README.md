# Local fixture viewer

From the Difftastic repository root:

```sh
cargo build --locked
python3 examples/review/viewer/build.py
python3 -m http.server 4173 --bind 127.0.0.1 --directory examples/review/viewer
```

Open http://127.0.0.1:4173/#02-review-175 to start with the `renameTo` example.

The left panel renders a real Git/Myers unified diff. The right panel renders
our generated domain with syntax-level highlighting, enclosing context, and
fold toggles. Navigation covers all eleven pinned PRs. Both panels use the
original source's token annotations for syntax colors; only the right panel
highlights novel syntax at token granularity.

Corresponding folds share one toggle; unmatched folds operate on one side.
Language-specific queries in `src/parse/fold_queries` discover bodies, collections,
and imports for Rust, Python, Go, and JavaScript/TypeScript (including JSX/TSX).
Ordinary argument and parameter lists are excluded. Other languages currently
produce no fold candidates; there is no heuristic fallback.

Fold metadata travels on Difftastic's syntax nodes. The existing matched-position
traversal projects matched nodes into paired folds and unmatched nodes into
one-sided folds. Ranges derive from Atom spans or List delimiters; flattened
parents retain a range override. Each matched capture emits its own fold;
adjacent folds are not grouped. Every fold retains its semantic `kind`, so the
frontend can choose defaults without interpreting placeholder text. All folds
remain in the domain, including those outside the current display windows.
The frontend decides which folds are visible.

Nested controls disappear under a collapsed parent. Body folds start expanded;
imports start collapsed. Expansion explicitly reveals the full interior.
Inline interiors have controls within the code. Collapsing preserves the exact
prefix/suffix and syntax colors, including UTF-8 byte coordinates. Nested
fold state survives parent collapse. Generated pseudocode remains unimplemented.
Run `node --test examples/review/viewer/inline-folds.test.mjs` for byte-range
projection checks.
Use “All source context” to inspect omitted lines, and “Wrap lines” to choose
wrapping versus horizontal scrolling. A blue ↳ row uses head indentation;
its tooltip contains both exact source lines. It means matched syntax, not a
claim that moving code into a conditional preserves behavior.

“Mark reviewed” stores progress in this browser only. “Download domain JSON”
exports the `DiffResult` domain, including `hunks` and `folds`, excluding
view state, baseline patch and derived layout. No JSON has been promoted to an
approved golden yet. Existing text assertions remain until visual review.

`build.py` verifies the original blob hashes, restores base/head fixture commits
in temporary local repositories, and calls the real CLI. `data/` is generated,
ignored by Git, and safe to rebuild. No network access is needed. The static
server is local; there is no backend write/publish endpoint.

The CLI also exposes:

```sh
difft review --repo REPO --base BASE --head HEAD --path FILE --format json
```

`--format json` encodes the domain itself. `--format viewer` additionally carries
disposable alignment/visibility data derived from that domain, so this small
frontend can reuse Difftastic's line layout without reimplementing it in JS.
The JSON encoder keeps Difftastic's fields, enum variants, token correspondence,
source contents and zero-based coordinates. Hunk line sets are sorted for
stable encoding. JSON is a transport encoding, not a second diff model.

Each `hunks` entry carries Difftastic's changed-line data and `context`.
Syntax context candidates are discovered once before hunk construction. Each raw
hunk selects paired, non-novel rows from the existing line alignment through a
shared index. One merge pass combines neighboring hunks whose ordinary padding
or selected syntax context overlaps. Context unions are deduplicated and compacted
once on the resulting hunks; `ContextRange` contains only `lhs` and `rhs` ranges.
