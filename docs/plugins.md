# Plugins

A plugin shapes how diffr shows a changed file. Every plugin implements the
same contract, [`wit/plugin.wit`](../wit/plugin.wit), and diffr loads and
runs every plugin the same way: the bundled ones (`plugins/<name>/`, compiled
into diffr) and WASM components in a folder the configuration points at. All
are configured the same way, in `[plugins]` ([config.md](config.md#plugins)),
and produce the same wire records ([streaming.md](streaming.md#plugins)).

This is a research prototype: a WASM plugin runs with full access to the
machine (see [Access](#access)). Run only plugins you trust.

## The plugin points

A plugin is made once, then a file passes three points, each in
`plugins.order`:

0. **New.** When diffr starts, before any output, it makes each enabled
   plugin's one instance for the run: it deserializes the plugin's options
   into the plugin's own options type and calls `new` with them. A plugin
   that cannot be made (options that do not deserialize, a summarizer without
   an API key, a component that does not compile or link) is a setup error:
   diffr writes it to stderr, naming the plugin as `plugins.<name>`, and exits
   2 before any record, for native and component plugins alike.
1. **Pre-process: `classify`.** Before the stream starts, each plugin's
   `classify` returns tags to add to the file's manifest entry. They join the
   tags from the bundled rules and git attributes
   ([config.md](config.md#file-tags)), so they decide how the file is diffed
   (a `generated` file gets a line diff), how `--order` ranks it, and what
   later plugins see: each plugin sees the tags the ones before it added. An
   error here, or a tag that is not lowercase letters, digits, `-` and `_`,
   is a setup error.
2. **During: queries.** The tree-sitter query files a plugin lists in its
   `plugin.toml` join the per-language fold query the diff runs with
   ([config.md](config.md#queries)). They decide which folds exist and tag
   them `<plugin>:<name>`. They are data, not code, and tags are the only
   syntax a plugin sees: the `context` plugin reads the scopes its queries
   tag `context:scope`.
3. **Post-process: `mutate`.** After the file is diffed and its region trees
   are built, each plugin's `mutate` reads the file and both sides and
   returns moves, which diffr carries out before the next plugin runs. An
   error, a trap, or a move that cannot be carried out aborts the run with
   `mutation_failed` naming the plugin.

## A plugin folder

```text
examples/plugins/fixtures/
  plugin.toml       # name, title, options schema, query files
  plugin.wasm       # the component
  queries/rust.scm  # optional: one query file per language
```

`plugin.toml` is the same file the bundled plugins carry
([config.md](config.md#plugin-folders)): `name` (the entry name in
`[plugins]`, and the prefix of every tag its queries set), `title`,
`description`, `[enabled]` (with `default`, whether the plugin is on unless
its entry says otherwise), `[options.<key>]` (each a JSON Schema with a
`title`), and `[queries]`. Query paths are relative to the folder, absolute,
or `builtin:<plugin>/<path>` for a bundled file such as
`builtin:shared/queries/rust.scm`.

A plugin from disk is an entry with `path`, relative to the configuration
file's directory (or absolute), and must be listed in `order`:

```toml
[plugins]
order = ["context", "hide-files", "deleted-bodies", "test-bodies", "removed-runs", "summarize", "group", "fixtures"]

[plugins.fixtures]
path = "plugins/fixtures"   # holds plugin.toml naming "fixtures"
fail = false                # an option from its plugin.toml
```

`enabled` and `path` are diffr's keys; every other key is an option,
validated against the folder's `plugin.toml` and filled with its defaults,
and passed to the plugin as one JSON object. The folder's `plugin.toml` must
be named after the entry.

### Loading

diffr reads every entry's folder the same way: a bundled plugin's embedded
folder, or the folder `path` names. Then:

- A folder holding `plugin.wasm` runs that component through wasmtime.
- Any other folder runs the native code diffr registers under the plugin's
  name: the bundled plugins, compiled in from the same crates that build
  their components. A folder with neither is a setup error.

So a bundled plugin's entry can set `path` to a folder holding a component,
which then runs in the native plugin's place with the folder's queries.

## The contract

[`wit/plugin.wit`](../wit/plugin.wit) is the contract: the package
`diffr:plugin@0.1.0`, world `plugin`. A plugin exports the interface `guest`,
which holds one resource, the plugin itself (the component model has no
optional exports; a plugin that does not classify returns an empty list):

```wit
resource plugin {
    new: static func(options: string) -> result<plugin, string>;
    classify: func(file: file-entry) -> result<list<string>, string>;
    mutate: func(file: file-entry, lhs: option<source>, rhs: option<source>) -> result<list<move>, string>;
}
```

`new` is a static function rather than a constructor, because a constructor
cannot fail.

- `options`: the plugin's entry as a JSON object, defaults filled in and
  already validated against `plugin.toml`. Only `new` gets it; what the
  plugin keeps from it lasts the run.
- `file-entry`: `file`, `status` and `tags`. `file` is the manifest entry's
  sides — `both`, `left-only` (a deletion) or `right-only` (an addition) — and
  each side is a `file-ref` of `path`, `oid` (the blob; empty outside a
  repository) and `mode`. `status` says how the two sides relate, which the
  sides alone do not: a path that changed is `renamed`, an object kind that
  changed `type-changed`. The SDK's `FileEntry::side()` is the side a file is
  named by and `path()` its path.
- `source`: one side's `text` and its `regions`, the tree flattened in
  preorder: each `region` carries its `parent` (a fold's `id`, or `0` for a
  top-level region), `id`, `fold-state-id`, `range`, `tags`, `visibility`, and
  `kind`: `leaf(alignment-id, changed)` or `fold`. The fields mean what they
  mean on the wire ([streaming.md](streaming.md#regions)). A binary file's
  sides have empty text and no regions.
- `move`: `cut({region, at})`, `join-folds(ids)`, `link-fold-state(ids)`,
  `set-collapsed((id, collapsed))`, `set-label((id, label))` and
  `set-tags((id, tags))`, with the semantics of the moves in
  [streaming.md](streaming.md#plugins). `0` names the file where a move
  allows it.

diffr builds these records once per call, from the file's manifest entry and
its trees as the plugins before left them, and hands the same records to a
native plugin and to a component. Nothing else reaches a plugin.

diffr makes one instance of each plugin per run and calls it for every file.
A native plugin's instance is called from several files' workers in parallel,
so it must be `Send + Sync`; interior mutability is the plugin's own
business. A component's instance lives in one wasmtime store, which runs one
call at a time: diffr calls a component plugin one file at a time.

### Fresh ids

A plugin often needs to name what its own moves create: the piece a cut
leaves, the fold a join adds. Ids are handed out deterministically, so a
plugin can predict them:

- When a plugin's moves begin, the next fresh `id` is one above the largest
  region `id` in the file (both sides), and the next fresh `alignment_id` one
  above the largest leaf `alignment_id`.
- A **cut** takes one fresh `alignment_id`, which the second pieces share,
  then one fresh `id` for the second piece on each side that holds the leaf
  or the leaf paired with it, lhs first. Both second pieces take the lhs
  piece's `id` (or the only piece's) as their `fold_state_id`. The first
  piece keeps the leaf's ids.
- A **join** takes one fresh `id` for the new fold on each side that holds
  the listed regions, lhs first; both share the lhs fold's `id` as their
  `fold_state_id`.
- No other move takes an id.

So cutting a paired leaf when the largest id is `n` names the lhs piece
`n + 1` and the rhs piece `n + 2`. diffr carries out every plugin's moves
with the SDK's applier (`crates/diffr-plugin-sdk/src/apply.rs`), the same
code a plugin predicts ids with.

## Host imports

Besides WASI, which only a component gets, diffr gives every plugin one
import, `diffr:plugin/host`:

- `git(args) -> result<string, string>`: runs `git` with these arguments in
  the repository's working directory (the current directory for
  `--no-index`), returning stdout on success and stderr otherwise.

A host failure (git cannot be started, or its output is not UTF-8) is the
call's error, natively and in a component alike.

A plugin does everything else itself. It reads files: a component has the
working directory preopened, and reads any side of a file the working tree
does not have with `git show`. And it prints what it wants to say to stderr,
rather than calling diffr to say it.

### Access

A component gets full access; diffr does not sandbox it:

- WASI with the working directory preopened read-write as `.`.
- The environment inherited.
- The network open, with name lookup.
- Its stdout and stderr captured, and written to diffr's stderr a line at a
  time, each line prefixed with `[<plugin name>] `. Neither may reach diffr's
  stdout, which is the stream. A native plugin writes to diffr's stderr
  directly, and is not prefixed.

## Writing a plugin in Rust

`crates/diffr-plugin-sdk` is the contract in Rust, and every bundled plugin is
written with it:

- `types`: the contract's records, generated from `wit/plugin.wit` itself by
  `wit_bindgen::generate!`, so there is one definition of each. A plugin
  works with the same records natively and as a component.
- `Plugin`: the one trait, mirroring the `plugin` resource. A plugin is a
  struct with an `Options` type (deserialized from the options JSON), and
  implements `new` (make the plugin from its options; where it can fail),
  `classify` and `mutate`. None has a default: a plugin that does not
  classify returns `Ok(Vec::new())`.
- `export!(MyPlugin)`: built for `wasm32`, exports the plugin as the
  component's `plugin` resource: its `new` deserializes the options string
  into `Options` and calls `Plugin::new`, and its `classify` and `mutate`
  call the instance with the records it was given, since the guest bindings'
  records are the contract's. Built for anything else, it expands to nothing,
  and diffr's native registry deserializes the options and calls the trait
  itself. The same source builds both ways.
- `host::git`: the host function, the same call natively and in a component.
- `tree::sides(lhs, rhs)` rebuilds the records as region trees
  (`Pairing<Source>`, each `Region` holding its children), and the helpers
  the bundled plugins read trees with: `walk`, `OtherSide`, `one_sided`,
  `docstring_of`, `before_and_after_ids`, and the rest.
- `Draft` carries moves out on a copy of the trees as the plugin makes them,
  with diffr's applier, so `draft.cut_lines`, `draft.collapse`, `draft.link`
  and `draft.group` can name what earlier moves created.
  `apply::Fresh::of(&sides)` predicts ids without a draft.

```toml
# Cargo.toml
[dependencies]
diffr-plugin-sdk = { path = "../../../crates/diffr-plugin-sdk" }
serde = { version = "1.0", features = ["derive"] }
```

```rust
use diffr_plugin_sdk::types::Source;
use diffr_plugin_sdk::{anyhow, export, FileEntry, Move, Plugin, ROOT};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {}

pub struct HideAll;

impl Plugin for HideAll {
    type Options = Options;

    fn new(_: Options) -> anyhow::Result<Self> {
        Ok(Self)
    }

    fn classify(&self, _: &FileEntry) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn mutate(
        &self,
        _: &FileEntry,
        _: Option<&Source>,
        _: Option<&Source>,
    ) -> anyhow::Result<Vec<Move>> {
        Ok(vec![Move::SetCollapsed((ROOT, true))])
    }
}

export!(HideAll);
```

Build it as a component with the pinned toolchain and the `wasm32-wasip2`
target, which emits a component directly:

```sh
rustup target add wasm32-wasip2
cargo rustc --release --target wasm32-wasip2 --crate-type cdylib
cp target/wasm32-wasip2/release/hide_all.wasm plugin.wasm
```

`scripts/build-wasm-plugins.sh` builds every plugin crate in this repository
that runs as a component into its folder's `plugin.wasm` (the outputs are not
committed). diffr compiles each component once per run, with wasmtime's
compilation cache on disk, so an unchanged component is not recompiled on the
next run.

## Examples

- `examples/plugins/fixtures`: `classify` tags a file `fixture` when it is
  under a `fixtures/` directory or its working-tree file starts with a
  `// fixture` line (read from the filesystem, so a deleted file is
  classified by its path alone); `mutate` hides a fixture behind the subject of the last
  commit that touched it (from `git log -1 --format=%s -- <path>`), and
  collapses all but the first line of its first leaf, naming the cut's piece
  by predicting its id. Its `fail` option makes `mutate` return an error.
- `plugins/context`, `plugins/hide-files`, `plugins/deleted-bodies`,
  `plugins/test-bodies`, `plugins/removed-runs` and `plugins/group`: the
  bundled plugins, which build as components too. Pointing their entries at
  the bundled folders once the script has built them runs the components in
  place of the native code, and produces the same stream (`tests/wasm.rs`
  checks it):

  ```toml
  [plugins.deleted-bodies]
  path = "/path/to/diffr/plugins/deleted-bodies"
  ```

  The summarizer's HTTP client does not build for `wasm32-wasip2`, so it runs
  natively only.
