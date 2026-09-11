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

The frontend launches Rust with `--format ndjson --syntax`: the wire is diffr's v2
protocol (`src/protocol.rs`) and `--syntax` adds a tree-sitter capture name per token,
which is the only source of syntax colour here.

## Themes

Colours come from Helix theme files: TOML keyed by tree-sitter capture names such as
`keyword`, `function.method`, `string`, `comment`, `type`, `variable.parameter`, with
a `[palette]` section and `ui.*` keys for chrome. A capture falls back to its parent
scope (`keyword.return` → `keyword`). Four themes are bundled under `themes/` (Helix's
own, MPL-2.0): `default-dark` (onedark), `default-light` (onelight), `gruvbox`, and
`solarized_light`.

The frontend reads `diffr config show --json` at startup and uses `theme.path` when set,
else `theme.name` from the bundled index. An unknown name is an error, not a fallback.
To use any other Helix theme, download it from
https://github.com/helix-editor/helix/tree/master/runtime/themes and point at it:

```sh
diffr config set theme.path ~/.config/helix/themes/dracula.toml
```

`t` toggles between `default-dark` and `default-light`. Change tints (line and word
backgrounds) are mixed from the theme's background and its `diff.plus`/`diff.minus`
colours. Recordings played with `--input` take `--theme <name|file.toml>` and default
to `default-dark`.

## Settings screen

`diffr config` opens a searchable settings screen in this frontend. The CLI contract it
relies on:

```sh
bun run packages/hunk/src/main.tsx --settings --diffr /path/to/diffr [initial query]
diffr config schema        # JSON Schema; each property carries `description` and `default`
diffr config show --json   # resolved values, nested like the schema
diffr config set <key> <value>   # dotted key, value as typed: true, 12, gemini
```

The list view: type to filter (fuzzy over keys, substring over descriptions), `↑`/`↓` to
move, Enter to edit, Esc to quit. `●` marks a value that differs from its default and an
empty value shows as `<unset>`. Enter opens an edit view for the selected setting with
its key, description and default: booleans and enums pick from a list (`↑`/`↓`, or `y`/`n`
for booleans), numbers and strings use a text field, masked for credential-looking keys
such as `api_key`. Enter saves through `diffr config set` and returns to the refreshed
list; Esc returns without saving. Both views carry a footer hint line.

## Controls

- Wheel, arrows, j/k, Page Up/Down, Home/End: scroll.
- Sidebar click: jump to a file in the continuous review stream.
- File header click or Enter: collapse/expand the file.
- `\` / Cmd-B (when forwarded by the terminal): toggle the file tree.
- Click folders to expand/collapse them; click files to navigate. The active file is highlighted and revealed as the diff scrolls.
- A summary strip under the menubar shows the comparison (`main…HEAD`, `index…working
  tree`), the file count, the totals `+N −M` in the theme's diff colours, and GitHub's
  five-block bar. Totals and file-header counts are the changed lines currently visible:
  diffr's `stats.visible` (its default fold state) adjusted as folds, gaps, and files are
  toggled, so lines hidden inside a collapsed region are not counted. While results are
  still arriving the total ends in `…`.
- `i` (or clicking the totals) opens a breakdown for the whole comparison and the current
  file: `visible`, `structural` (from tree-sitter, or `line diff` when it fell back), and
  `textual`. Esc closes it.
- A file diffr marks hidden by default (generated, test) opens collapsed with a GitHub-style
  placeholder: `Load diff` and the reason line. Click it or press Enter to reveal.
- File, View, Navigate, Theme and Help menus expose the supported controls.
- `[` / `]`: previous/next hunk.
- Folds follow VS Code with controls always shown: a foldable row shows `▾` in the
  gutter, and a collapsed fold shows `▸` plus `⋯ Label` after the header line. A label
  with several lines (pseudocode from a summarizer) shows only `⋯` on the header and hangs
  the label under it, indented one level, inside the fold tint. Click either to toggle.
  Alt-click also folds or unfolds every nested region. Vim chords act on the fold whose
  header is the top row: `za` toggle, `zo` open, `zc` close, with `zA` / `zO` / `zC`
  recursive; `zM` / `zR` (and View > Fold all / Unfold all) fold or unfold every fold;
  `zj` / `zk` scroll to the next or previous fold header. The header and closing
  delimiter stay visible; regions that share an id collapse on both sides. A fold on one
  side only blanks that side's cells, keeping the other side's lines in Rust's alignment.
- Context gaps are folds too: an unchanged run diffr trimmed to N lines around changes
  arrives as a collapsed leaf tagged `unchanged`, and renders as one fold row with its
  label (`142 unchanged lines`) that toggles like any other. `c` opens or closes every
  such gap at once. Folds and gaps start in the state diffr's `visibility` asks for.
- `s`: split/unified; initial mode is responsive to width.
- `w`: wrap; Left/Right: horizontal scrolling when unwrapped.
- `t`: toggle between the bundled dark and light defaults (see Themes).
- Drag code rows: select original source lines on the starting side; `y` or Copy copies
  via OSC 52. Escape clears selection. Character-level selection and drag autoscroll
  are not implemented in this first pass.
- `q` / Ctrl-C: quit.

## Ownership and data flow

```text
Rust CLI -- implicit interactive output --> Bun frontend
Bun frontend -- same comparison arguments + --format ndjson --syntax --> Rust subprocess
Rust stdout --> validated events --> file store
per-side text + syntax spans + region trees --> leaves zipped by id --> split/unified rows
rows + width + wrapping --> measured row bounds
row bounds + viewport --> mounted OpenTUI rows
mouse/keyboard --> viewer state (collapsed ids, closed files) --> updated projection
```

- `src/cli.rs` owns argument interpretation and selecting interactive versus explicit
  output. The frontend does not resolve revisions or invoke Git.
- `src/protocol.rs` is the wire contract: a `start` manifest, one `file` record per file
  with `diff` or `error`, and a `complete` footer. Sides are `lhs`/`rhs` by presence.
  Each text side carries its full text, optional `syntax` spans, and a `regions` tree
  whose leaves tile the file; the same region `id` on both sides means correspondence.
- `packages/hunk/src/diffr/wire.ts` validates those shapes with Zod and fills omitted
  defaults. Columns remain zero-based UTF-8 byte offsets.
- `diffr/regions.ts` flattens each side's tree into leaves and folds, computes which lines
  a collapsed fold hides (a trailing line hides only when nothing follows the range on
  it), and seeds the default collapsed set from `visibility`. Collapsed ids live in
  viewer state, keyed by region id, so paired regions toggle together.
- `diffr/stream.ts` validates event ordering, versions and completion counts, handles
  arbitrary chunk boundaries, and rejects truncated streams.
- `diffr/store.ts` holds completed files and errors. The UI can display files while
  subsequent results are arriving. Quit terminates the comparison subprocess.
- `diffr/rows.ts` zips the two leaf lists on their ids: paired leaves pair rows line for
  line, unpaired leaves get blank cells opposite, and a leaf whose partner already went
  by is a move, shown one-sided on each side. Folding masks lines per side; nothing is
  realigned. Line tint comes from a leaf's `changed` spans, word emphasis from the spans
  themselves, and foreground colour from `syntax` capture names through a small theme
  table. There is no patch parser, second diff algorithm or frontend tokenizer.
- `diffr/config.ts` and `ui/Settings.tsx` implement the settings screen over the
  `diffr config` commands.
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

Copying the hidden lines of a collapsed fold, character-level source selection, drag autoscroll, full Hunk theme
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

Alignment is supplied by Rust as per-side region trees whose leaves share ids across
sides. Context selection arrives as collapsed `unchanged` leaves; `[` and `]` jump
between runs of changed rows. Saved streams from the v1 wire must be regenerated.



Drag the vertical divider at the right edge of the tree to resize it. The chosen
width survives hiding and reopening the tree and is clamped on terminal resize.

Navigation uses Hunk's key matcher and defaults: j/k or arrows scroll lines;
d/u or Ctrl-D/Ctrl-U scroll half pages; f/b, PageDown/PageUp, or Ctrl-F/Ctrl-B
scroll full pages; Space/Shift-Space also page; g (or gg)/G go to start/end.
h/l or arrows pan horizontally. Cmd-B toggles the tree; backslash is its fallback.

The first NDJSON event includes `files` in comparison order, each with its status,
category, language and default visibility.
The tree renders this manifest immediately; pending files are marked ◌ and failures !.
Diffs appear in tree order as they arrive, retaining the visible source row when
an earlier file loads. Result arrival order need not match manifest order; each
manifest file must produce exactly one result or file error.
Selecting a pending file jumps to its diff when it arrives. Rust emits and flushes
the manifest before structural comparison, including for `--no-index`.
