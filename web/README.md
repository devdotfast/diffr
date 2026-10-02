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
`http://localhost:5173/devdotfast/whiteboard/pull/837`.

The rows, folds and themes come from the TUI's model in `tui/packages/hunk/src/diffr`, unchanged;
`vite.config.ts` adapts the Bun-specific parts (TOML theme imports, terminal text width).

Plugins: the bundled native plugins run as they do in the CLI. WASM component plugins need the
native diffr for now.
