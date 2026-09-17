# diffr

## Installation

```sh
cargo xtask install
```

Run this from the checkout with Rust and [Bun](https://bun.sh) on `PATH`.

## Configuration

To turn on semantic diff summarization:

1. Run `diffr config`
2. Search for 'summarization'
  - Enable in the dropdown
  - Add your API key for Gemini if you don't already have on your path

## Usage

`diffr` accepts the exact same arguments that `git diff` does.

```sh
diffr                         # index versus working tree
diffr --cached                # staged changes
diffr main...HEAD -- src/      # merge-base comparison
```

You can also use `diffr` in streaming mode, which is useful for TUI or GUI applications:

```sh
diffr main HEAD --format ndjson
```

## Architecture

When you run `diffr ${commit_range_exp}`, the following happens:

1. Commits loaded from git
2. Plugins (explained in more detail later) load
3. Each file is parsed via tree-sitter & diffed using difftastic's ast/ast diffing algorithm
  - This produces an alignment of file / file
  - Note: because of known upstream limitations, the diffing algorithm is quite CPU/Mem intensive.
    We fall back to a textual diffing algorithm in case of issue
4. Plugins define which AST nodes are present in the API + folded by default.

### Plugin API

`diffr` has a powerful, wasm-based plugin API which customizes how it presents changed files.

For example, the following are all implemented as plugins:

- Context Folding: showing relevant context, like a function signature + closing brace (if applicable)
- Algorithm Summarization: using an LLM to summarize long algorithms into pseudocode
- Comment collapsing: collapsing long LLM comments + function bodies & expanding both at once
- Collapsing tests by default

The [Rust SDK's `Plugin` trait](crates/diffr-plugin-sdk/src/lib.rs) exposes
four important methods:

```rust
// rust bindings of underlying WASM plugin API
use diffr_plugin_sdk::{FileEntry, Move, Pairing, QuerySource, Source};

pub trait Plugin: Sized {
    /// Options are configuration for the plugin
    type Options: serde::de::DeserializeOwned;

    /// new loads the plugin from its configuration; this is to allow plugins to fail
    /// early if user config isn't set correctly
    fn new(options: Self::Options) -> anyhow::Result<Self>;

    /// queries return tree-sitter queries to add metadata to the tree-sitter tree
    /// this means that the plugins can backpack off of the tree-sitter parse that the
    /// diffing algorithm does.
    fn queries(&self) -> anyhow::Result<Vec<QuerySource>>;

    /// classify (bad name lol) runs classification of files into generated, test, etc.
    /// Useful to prevent wasteful semantic diffing for things users will skip.
    /// emits tags that clients can make use of
    fn classify(&self, file: &FileEntry) -> anyhow::Result<Vec<String>>;

    /// mutate emits a series of structured mutations ('Moves') to the parsed diff type
    /// (e.g., fold X function body, show Y lines of context around it, etc.)
    fn mutate(&self, file: &FileEntry, sides: &Pairing<Source>)
        -> anyhow::Result<Vec<Move>>;
}
```

```mermaid
sequenceDiagram
    participant D as diffr
    participant P as Plugins
    participant G as Git
    participant E as Diff engine
    participant C as UI / API consumer

    D->>P: new(options), queries()
    P-->>D: Plugin instances and query sources
    D->>G: Load changed files for comparison
    G-->>D: Before and after versions
    loop Each changed file
        D->>P: classify(file)
        P-->>D: File tags
        D->>E: Compare versions using tags and queries
        alt Structural comparison available
            E->>E: Parse with tree-sitter and diff with difftastic
        else Generated file or structural fallback
            E->>E: Compute line diff
        end
        E-->>D: Aligned regions and folds
        loop Each enabled plugin in order
            D->>P: mutate(file, sides)
            P-->>D: Presentation moves
            D->>D: Apply moves to regions and fold state
        end
        D-->>C: Diff with initial fold state
    end
```

The WASM interface is defined in [`wit/plugin.wit`](wit/plugin.wit).
The SDK's [`export!` macro](crates/diffr-plugin-sdk/src/lib.rs) and
[guest adapter](crates/diffr-plugin-sdk/src/guest.rs) expose a Rust plugin
as a WASM component; the [Wasmtime runner](src/plugin/wasm.rs) loads and
calls it. See the [context plugin](plugins/context/src/lib.rs) and its
[Rust query](plugins/context/queries/rust.scm) for a concrete implementation,
or [Writing plugins](docs/plugins.md) for the full guide.

## License

This project was forked from the lovely [difftastic](https://github.com/wilfred/difftastic).
