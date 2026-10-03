# Plugin Architecture

`diffr` has a powerful, wasm-based plugin API which customizes how it presents changed files. For more details, read the [docs](./docs/plugin.md).

For example, the following are all implemented as plugins:

- Context Folding: showing relevant context, like a function signature + closing brace (if applicable)
- Algorithm Summarization: using an LLM to summarize long algorithms into pseudocode
- Comment collapsing: collapsing long LLM comments + function bodies & expanding both at once
- Collapsing tests by default

## Architecture

When you run `diffr ${commit_range_exp}`, the following happens:

1. Commits loaded from git
2. Plugins (explained in more detail later) load
3. The classifier tags each changed file: generated, vendored, docs, test, or a custom tag
4. Each file is parsed via tree-sitter & diffed using difftastic's ast/ast diffing algorithm
  - This produces an alignment of file / file
  - Note: because of known upstream limitations, the diffing algorithm is quite CPU/Mem intensive.
    We fall back to a textual diffing algorithm in case of issue
5. Shape plugins define which AST nodes are present in the API + folded by default.

## Plugin Interface

The WASM interface is defined in
[`crates/diffr-plugin-sdk/wit/plugin.wit`](../crates/diffr-plugin-sdk/wit/plugin.wit).
It has two kinds of plugin: one classifier, which tags each changed file, and
shape plugins, which run in order on each diffed file.

A classifier implements the generated `GuestClassifier` trait:

```rust
use diffr_plugin_sdk::{Classification, ClassifierGuest, FileEntry, GuestClassifier, Tag};

struct MyClassifier;
impl ClassifierGuest for MyClassifier {
    type Classifier = Self;
}
impl GuestClassifier for MyClassifier {
    fn new(options: String) -> Result<Self, String> { Ok(Self) }
    fn classify(&self, file: FileEntry) -> Result<Classification, String> {
        Ok(Classification { tags: vec![Tag::Custom("schema".into())], hidden: None })
    }
}
diffr_plugin_sdk::export_classifier!(MyClassifier);
```

A shape plugin implements the generated `GuestPlugin` trait:

```rust
use diffr_plugin_sdk::prelude::*;

struct MyPlugin;
impl Guest for MyPlugin {
    type Plugin = Self;
}
impl GuestPlugin for MyPlugin {
    fn new(options: String) -> Result<Self, String> { Ok(Self) }
    async fn visit(&self, cursor: &Cursor, phase: Visit) -> Result<bool, String> {
        // Inspect this node and edit it through the host cursor.
        Ok(true)
    }
}
export!(MyPlugin);
```

The [host](../src/plugin/wasm.rs) constructs the plugin and drives traversal.
See the [context plugin](../plugins/shape/context/rust/src/lib.rs) and its
[Rust query](../plugins/shape/context/queries/rust.scm) for a concrete
implementation, and the [classifier](../plugins/classify/rust/src/lib.rs).
