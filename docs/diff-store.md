# Computed diff storage

Storage is a host-owned diffr setting. JS, Review API payloads, and MCP inputs
never name a backend or storage path. Configure it with the existing CLI:

```sh
diffr config set storage.backend sqlite
diffr config set storage.path .cache/diffr
```

Or in diffr's configuration file (`$XDG_CONFIG_HOME/diffr/config.toml`, normally
`~/.config/diffr/config.toml`):

```toml
[storage]
backend = "sqlite" # "memory" (default), "file", or "sqlite"
path = ".cache/diffr"
```

`storage.path` is a directory, relative to the source repository unless absolute.
File storage writes entries there; SQLite uses `diffs.sqlite` inside it. Memory
ignores the path. Persistent directories are created automatically. File entries
are published with atomic rename; SQLite uses WAL and a busy timeout.

The native Rust entry points load diffr config and select the store. Configuration
is checked on each call, and a changed setting selects a runtime with that config.
Storage settings are excluded from computed-diff keys: moving the backing store
does not change analysis identity. Analysis uses the same loaded diff/plugin
configuration. All native reads, writes, setup, and computation run on N-API's
blocking worker pool. JS simply calls `hydrate(scope, hits)` and
`postprocess(scope, results)` as before.

Rust callers can provide their own synchronous store:

```rust
use std::sync::Arc;
use difftastic::{storage::FileStore, search::{Index, Session}};

let store = Arc::new(FileStore::open(".cache/diffr")?);
let index = Arc::new(Index::new(store));
let mut session = Session::new(scope, index.clone())?;
let candidates = session.hydrate(hits)?;
let results = session.postprocess(candidates)?;
// Other comparisons use the same index.clone().
# Ok::<(), anyhow::Error>(())
```

`search::store::Store: Send + Sync` has `get`, `put`, and `remove` methods, using `DiffKey`
and `StoredDiff`. `StoredDiff` supports serde for custom backends. Built-in
implementations are `storage::MemoryStore`, `storage::FileStore`, and `storage::SqliteStore`.

Each entry contains the computed base/head source trees, source text, syntax,
alignments, changed spans, file identities, and classification metadata. It has
no search highlights or query-specific presentation state. Hydration and
postprocessing work on copies. Jev responses are not stored here.

Keys hash canonical JSON containing the engine/package and cache-format version,
analysis configuration, resolved query contents (including imports), paths,
blob IDs, modes, status, and classification tags. Different queries over the same
files reuse the computed trees. Different analysis settings produce different
keys. Worktree pins are validated even on a warm cache. Developers must bump
`CACHE_VERSION` in `src/search/store.rs` when algorithms, parsers, or tree semantics
change incompatibly without a package version bump.

`Index<S: Store>` owns the shared store and performs cache lookup and diff
computation. Each `Session` owns its pinned scope, compiled queries, plugin
pipeline, and comparison manifest, and holds a reference to the same index.
Sessions are created per native call; dropping one does not discard computed
diffs. There is no cache of indexes keyed by scope or plugin configuration.

The host holds one active index. Changing commits or analysis settings creates
new session state while keeping that index and store. Only changing the storage
backend or resolved directory replaces the active index; existing sessions can
finish with their original index. Memory ignores the configured path and is
shared across repositories. For persistent storage, a repository-relative path
resolves to that repository's directory.

Hydration and postprocessing use typed Rust inputs; only the N-API boundary
converts JSON. Sessions do not share mutable plugin state or serialize all search
operations through a global lock. Concurrent cold misses may compute the same
diff independently; writes atomically replace the entry for that key.

Store errors and corrupt records are reported to the caller. No automatic
retention/eviction policy is included: memory entries live until removed or the
index/store is dropped; persistent entries live until removed. Old-version
entries can be deleted. Replacing the active memory backend discards its entries once existing sessions finish.

Rust tests load real configuration files and reopen persistent stores with writes
forbidden to verify cache reuse, identical results, configuration isolation, and
absence of query highlights in saved entries. Store contract tests cover replacement/removal, reopen, corruption,
canonical keys, and concurrent file writers.
