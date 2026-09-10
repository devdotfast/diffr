# diffr terminal frontend

This is the first terminal frontend adapted from Hunk. Rust computes the comparison;
OpenTUI/React displays it. Pierre and Shiki are not dependencies.

## Run locally

From the repository root:

```sh
(cd tui && bun install)
cargo build --bin diffr
./target/debug/diffr main HEAD
./target/debug/diffr --no-index -- before.ts after.ts
```

With terminal stdin and stdout, omitting `--format` opens the viewer. Redirected output
stays text. Explicit `--format text`, `json`, `ndjson`, or `snapshot`, and metadata/quiet
modes bypass the frontend. `DIFFR_BUN` can specify a Bun executable; `DIFFR_TUI_ENTRY`
can specify a frontend entry file. The default entry is in the checkout used to build
Rust. A standalone distribution/installer is deferred.

Saved streams use the same reader:

```sh
./target/debug/diffr main HEAD --format ndjson > /tmp/comparison.ndjson
(cd tui && bun run start --input /tmp/comparison.ndjson)
```

`--input -` reads a pipe and opens the controlling terminal for keyboard input.

## Controls

- Wheel, arrows, j/k, Page Up/Down, Home/End: scroll.
- Sidebar click: jump to a file in the continuous review stream.
- File header click or Enter: collapse/expand the file.
- `[` / `]`: previous/next hunk.
- `s`: split/unified; initial mode is responsive to width.
- `w`: wrap; Left/Right: horizontal scrolling when unwrapped.
- `t`: dark/light theme.
- Drag code rows: select original source lines on the starting side; `y` or Copy copies
  via OSC 52. Escape clears selection. Character-level selection and drag autoscroll
  are not implemented in this first pass.
- `q` / Ctrl-C: quit.

## Ownership and data flow

```text
Rust CLI -- implicit interactive output --> Bun frontend
Bun frontend -- same comparison arguments + --format ndjson --> Rust subprocess
Rust stdout --> validated events --> file store
file source + token spans + hunk pairs --> split/unified rows
rows + width + wrapping --> measured row bounds
row bounds + viewport --> mounted OpenTUI rows
mouse/keyboard --> viewer state --> updated projection
```

- `src/cli.rs` owns argument interpretation and selecting interactive versus explicit
  output. The frontend does not resolve revisions or invoke Git.
- `src/stream.rs` owns serialization. Repository comparisons retain their existing
  wire format. Standalone comparisons add start-event operands `{kind: "file", path}`.
- `packages/hunk/src/diffr/wire.ts` validates the Rust field shapes. Source positions
  remain zero-based UTF-8 byte offsets. Fold metadata, tags, pairings and placeholders
  are retained intact, but no structural fold state is applied yet.
- `diffr/stream.ts` validates event ordering, versions and completion counts, handles
  arbitrary chunk boundaries, and rejects truncated streams.
- `diffr/store.ts` holds completed files and errors. The UI can display files while
  subsequent results are arriving. Quit terminates the comparison subprocess.
- `diffr/rows.ts` consumes the exact hunk line pairs. It adds empty split cells where
  Rust supplies null, and groups unified removals before additions between shared
  context lines. Syntax colors and novelty emphasis come from Rust token spans.
  There is no patch parser, second diff algorithm or frontend syntax highlighter.
- `diffr/geometry.ts` measures wrapping and equal-height split rows. Retained Hunk
  `styledSpanLayout.ts`, `ui/lib/text.ts` and `rowWindowing.ts` handle styled text
  slicing, terminal column measurement and binary-search viewport selection.
- `ui/diff/CodeRowView.tsx` paints already-measured cells. `ui/App.tsx` owns the current
  layout, file expansion, viewport, source selection and mouse/keyboard coordination.
- `diffr/selection.ts` copies original source lines, excluding gutters and padding.

Only viewport rows are mounted; source data and measured bounds for loaded files remain
in memory. This is not a bounded-memory file loader. Large-stream performance beyond
basic bounded-mount tests still needs profiling. Layout is recomputed on resize/wrapping
changes, with a source-row anchor used to retain scroll position where possible.

## Deliberately deferred

Interactive structural folds (including inline folds and paired toggles), expansion of
omitted hunk context, character-level source selection, drag autoscroll, full Hunk theme
catalog, distribution packaging, and shared web presentation code. The first pass has
no broker, agent sessions, annotations, extensions, VCS adapters, or alternate Pierre path.

The source paths still contain `packages/hunk` to make the import/refactor diff easy to
review. `TUI_UPSTREAM.md` in the repository root records the upstream revision; `LICENSE`
retains Hunk's MIT notice. The parent Rust project keeps its existing license.

## Verify

```sh
cargo build --bin diffr
cargo test --bin diffr
python3 tests/streaming/check.py
cd tui
bun run typecheck
bun test packages
bun run test:integration
```

The integration tests include real Rust wire output, redirected CLI behavior, and a
Unix PTY test for interactive launch, layout switching, mouse file toggles and clean
shutdown. They require Python 3 and the debug Rust binary. Set `DIFFR_TEST_BIN` to test
another binary. OpenTUI tests verify drag-copy, split/unified rendering, and bounded
mounted widgets while scrolling a 5,000-line file.
