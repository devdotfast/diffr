# diffr web

diffr in the browser: open a GitHub pull request or comparison and diffr diffs it on your machine,
with the engine compiled to WebAssembly. There is no server. The page fetches files straight from
`api.github.com` and `raw.githubusercontent.com`, and sends nothing anywhere else.

Public repositories need no token. A token lifts GitHub's limit of 60 API requests an hour and
opens private repositories; it is kept in the browser's localStorage and sent only to
`api.github.com`.

## Running it

```sh
rustup target add wasm32-unknown-unknown
rustup component add llvm-tools
cargo install wasm-bindgen-cli --version 0.2.129
cargo xtask build-web      # builds crates/diffr-web and writes its bindings to web/src/wasm
cd web && bun install && bun run dev
```

Then open `/owner/repo/pull/123` or `/owner/repo/compare/base...head`, for example
`http://localhost:5173/devdotfast/whiteboard/pull/837`. Keys: F2 toggles the diff stats, F3 the
engine panel, `/` searches the file tree.

The page is laid out after [DiffsHub](https://diffshub.com): the file tree is
[@pierre/trees](https://trees.software), and each file is drawn in the DOM that
[@pierre/diffs](https://diffs.com) styles, filled from diffr's own record, so the changed spans,
pairing, syntax colour and folds are diffr's. Files are windowed: every file keeps its height so
the scrollbar is true, but only those near the viewport hold rows.

Every file shows GitHub's own patch, from the file listing, as soon as the first page of the
listing arrives, coloured by diffr's own highlighter on a worker of its own, which reads the
patch's lines as a fragment of the file. diffr takes over lazily: files on or near the screen are fetched in full and
diffed, nearest first, and each one replaces its patch in place; scrolling keeps the file being
read where it is while the ones around it change height. Files GitHub lists without a patch (too
large) hold space for their changed lines until they come into view.

The engine runs in a pool of workers that share one compiled module. They start at two and grow to
eight while files queue, and all but one are let go after ten idle seconds, since wasm memory
never shrinks. Workers hand the page each file's text as one string with line offsets and its
highlights packed into typed arrays, so a change of a million lines stays a few hundred megabytes.

The fold model comes from the TUI's code in `tui/packages/hunk/src/diffr`, unchanged;
`vite.config.ts` adapts the Bun-specific parts (TOML theme imports, terminal text width).

GitHub lists at most 3000 files of a pull request; a larger one shows its first 3000.

Plugins: the bundled native plugins run as they do in the CLI. WASM component plugins need the
native diffr for now.
