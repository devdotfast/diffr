# Diffr code-mode API proposal

The implementation is available through `diffr/api`, backed by the shared Rust
engine and a Node-API addon. Code mode is ordinary Node.js or Bun with top-level
await. This document records the API contract and the agreed type changes.

## Shared Rust types

Reuse the existing `Source`, `Region`, and `Node` types. Add one optional serialized
field, `search_highlights`, to `Node::Leaf`; no separate search tree types or generic
payload parameter are needed. `Source` and `Region` keep their existing fields.

The only permitted change to the `Node` declaration is this field addition in
`src/protocol/mod.rs`. Existing fields, variants, attributes, and comments remain
unchanged; `Source` and `Region` declarations do not change.

```diff
diff --git a/src/protocol/mod.rs b/src/protocol/mod.rs
--- a/src/protocol/mod.rs
+++ b/src/protocol/mod.rs
@@ -236,6 +236,8 @@ pub enum Node {
         alignment_id: u32,
         #[serde(default, skip_serializing_if = "Vec::is_empty")]
         changed: Vec<Span>,
+        #[serde(default, skip_serializing_if = "Vec::is_empty")]
+        search_highlights: Vec<Span>,
     },
     /// A foldable region. Its range is the hull of its children.
     Fold { children: Vec<Region> },
```

Ordinary diffs leave `search_highlights` empty, so their serialized output remains
unchanged. Deserializing older data defaults the field to an empty vector. Rust
leaf constructors and exhaustive patterns must be updated for the new field.
Full text and syntax live once per source; regions address that text with the
existing zero-based UTF-8 byte coordinates and `Span` type.

An unchanged search match is valid without splitting its containing leaf:

```rust
Node::Leaf {
    alignment_id: 7,
    changed: vec![],
    search_highlights: vec![Span {
        line: 79,
        start_column: 4,
        end_column: 14,
    }],
}
```

`changed` means structural-diff markings; `search_highlights` means query-match coverage.
The two sets are independent. Highlights may occur in an unchanged region or a
wholly unchanged file. Search indexing reads unchanged blobs from Git and builds
syntax and fold structure in the search flow. The ordinary diff engine's
identical-file fast path remains unchanged.

During search hydration, `Source.regions` is a candidate vector, not necessarily
a deduplicated, file-tiling tree. Ordinary diffs retain their existing file-tiling
invariant. Hydration may preserve repeated
or overlapping regions as separate candidates for ranking and filtering. Each
candidate retains its own highlights and internally ordered region subtree.

Only after client selection does postprocessing merge compatible
selected candidates and union their highlights for rendering. It must not merge
away distinct candidates before the client has had the opportunity to rank them.
Within a candidate, highlights spanning leaves are partitioned among those leaves;
folds derive their information from descendants.

Proposed JS methods on bound search regions derive their answers from the leaves:

```ts
region.hasChanges(): boolean
region.hasHighlights(): boolean
region.hasChangedHighlights(): boolean // Highlight/change span intersection.
```

No stored `region_changed` or `highlights_changed` flags. The client can sort/filter
`Source.regions` directly. Preserve source order within each candidate's
subtree; postprocessing restores display order when combining selected candidates.

## Client inputs and signatures

```ts
interface Scope {
  repo: string;
  baseWorktree: { commitId: string; path: string };
  headWorktree: { commitId: string; path: string };
}

interface Hit {
  file: string; // Absolute path in either worktree.
  lines: {
    line: number; // 1-based, like rg -n and nl.
    text: string; // Source line without its line terminator.
  }[];
}

export function hydrate(scope: Scope, hits: Hit[]): Promise<SearchResult[]>;

export function postprocess(
  scope: Scope,
  selected: SearchResult[],
  options?: { plugins?: PluginConfig },
): Promise<SearchResult[]>;
```

`SearchResult` is the proposed client envelope around the paired search sources,
not an existing Rust wire type. Each `Source` holds one side of one file;
its regions can contain multiple hits and structural changes. `SearchResult`
groups the base/head sources for a file correspondence (or just one side), adds
file identity and scope, and provides printing and serialization methods. It is
not an individual hit or region. Its fields are shown explicitly here:

```ts
interface SearchResultData {
  kind: "combined" | "lhs" | "rhs" | "unchanged";
  scope: Scope;
  file: Pairing<FileRef>;
  sources: Pairing<Source>;
}

interface SearchResult extends SearchResultData {
  setCollapsed(foldStateId: number, collapsed: boolean): void;
  toString(): string;
  [inspect.custom](): string;
  toJSON(): SearchResultData;
}
```

`Pairing` and `FileRef` refer to bindings of the existing Rust types: pairings
serialize as `{ lhs, rhs }`, `{ lhs }`, or `{ rhs }`; FileRef contains path, oid,
and mode. Source binds the existing Rust type with the leaf extension above. `inspect` is Node's util
inspection symbol. Binding/package names and plugin options remain provisional.

`setCollapsed` sets `visibility.collapsed` on every region with the given
`fold_state_id` across both included sides of this result, preserving labels and
other visibility fields. It traverses nested regions and throws if the ID does
not exist. Descendants with other IDs retain their collapse state; opening a
nested region does not automatically open its ancestors. The helper mutates only
this result's visibility, without rerunning plugins or changing highlights.

- Scope is plain caller-supplied data. Both worktrees are required and pinned.
- Rust lazily indexes pinned Git blobs and caches structural analysis and
  query-derived regions before attaching hit-dependent visibility. There is no
  separate scope-resolution call. Custom postprocess plugin settings rebuild
  the relevant trees with those plugins' queries before applying selected spans.
- Hydration maps hits to the indexed source and attaches highlights. The current
  line-only Hit input produces whole-line highlights; token highlights would require
  richer search input later. A source counterpart isn't highlighted unless it matched.
- Hydration rejects with an error if a hit path is outside the scoped worktrees,
  does not resolve to an indexed source, or contains an out-of-bounds line number.
  Each hit line's text must exactly match the corresponding line in
  `Source.text`, excluding its line terminator; a mismatch also rejects
  hydration with an error.
- The old `SourceSelection` / `DiffSelection` / `matches` payload proposal is replaced
  by the search tree. Highlight spans cover matched lines; existing region
  visibility describes what to show. No parallel Coverage or visible-range schema.
- Combined versus side-only results use indexed correspondence, including renames,
  not filename equality. Unchanged is a valid result category on either side.
- Hydration initially exposes selected evidence without surrounding context.
- `postprocess` deduplicates compatible selected candidates, coalesces overlapping
  regions, unions their highlights, and applies configured plugins for context and
  folding. Preserve change spans and deliberate combined/side-only views. Splitting
  leaves for presentation is allowed but not necessary merely to represent a hit.
- Postprocessed sources retain the full indexed region tree for each included side.
  Unselected unchanged content outside plugin-expanded context is collapsed, not
  discarded, including before and after the excerpt. Callers can expand it by
  editing visibility and printing again; this
  does not add query highlights or require another read or hydration call.
- Both operations return `SearchResult[]`. Ordinary JS or a relevance scorer such
  as Jev can rank/filter results before or after postprocessing; no type conversion
  is required. Query-specific scores belong to the caller's ranking logic.
- Initial results can reference shared source content in memory. Do not independently
  mutate shared visibility trees when constructing distinct views.
- Plugins run on the coalesced full region trees, not the overlapping candidate
  vectors. Both ordinary diffs and search use the same plugin pipeline.

## Plugin contract and behavior

Carry `search_highlights: Vec<Span>` through the plugin SDK's leaf type and add
`search-highlights: list<span>` to the WIT leaf record. Conversions and plugin
moves must preserve both span sets, partitioning them when leaves split and
retaining them when regions are grouped. The WIT record change requires updating
and rebuilding affected plugin components; JSON omission does not make the plugin
ABI unchanged.

Add `Unchanged` / `unchanged` to the Rust and plugin file-status enums so wholly
unchanged files can run the same `classify` and `mutate` hooks with accurate metadata.

Update existing plugins to honor both changed regions and search-highlighted
regions. Context expansion uses the union of changed and highlighted lines as
anchors for neighboring lines and query-derived scope boundaries. No separate
search mode is required: an ordinary diff's search highlights are empty. Changes
away from search matches can therefore remain visible too.

Existing policies may still fold changed code. Search highlights override those
policies: the SDK move applier prevents file, ancestor, and linked-state collapse
from hiding a match. Grouping treats highlighted regions as boundaries, removed
runs skip highlighted leaves, and summarization excludes affected bodies and
linked docstrings before requesting a summary. Diff statistics and change coloring
continue to use only `changed`; search ranking and match coloring use
`search_highlights`. Shared SDK helpers can provide the union for visibility
policies without conflating the two meanings.

### Highlight fields at the plugin boundaries

The protocol `Node::Leaf` addition is shown above. These are the corresponding
SDK and WIT additions; no new span type is required:

```diff
diff --git a/crates/diffr-plugin-sdk/src/tree.rs b/crates/diffr-plugin-sdk/src/tree.rs
--- a/crates/diffr-plugin-sdk/src/tree.rs
+++ b/crates/diffr-plugin-sdk/src/tree.rs
@@ -73,6 +73,7 @@
     Leaf {
         alignment_id: u32,
         changed: Vec<Span>,
+        search_highlights: Vec<Span>,
     },
     Fold {
         children: Vec<Region>,
```

```diff
diff --git a/wit/plugin.wit b/wit/plugin.wit
--- a/wit/plugin.wit
+++ b/wit/plugin.wit
@@ -104,6 +104,7 @@
         /// lines up with this one.
         alignment-id: u32,
         changed: list<span>,
+        search-highlights: list<span>,
     }
 
     /// A leaf tiles the file; a fold's range is the hull of its children.
```

The WIT generates the Rust `types::Leaf` records; do not hand-edit generated
bindings. Each conversion must copy `search_highlights` alongside `changed`:

| Boundary | Required update |
| --- | --- |
| `src/plugin/mod.rs`: `to_tree` | Destructure protocol `search_highlights`; map every span's `line`, `start_column`, and `end_column` into SDK spans. |
| `src/plugin/mod.rs`: `from_tree` | Destructure SDK `search_highlights`; map the same three coordinates back into protocol spans. |
| `crates/diffr-plugin-sdk/src/tree.rs`: `Source::from_record` | Add `search_highlights: leaf.search_highlights.clone()` to the reconstructed leaf. |
| `crates/diffr-plugin-sdk/src/tree.rs`: `Source::to_record` | Destructure the field and add `search_highlights: search_highlights.clone()` to the flattened `types::Leaf`. |
| `src/plugin/wasm.rs`: `source` | Map `leaf.search_highlights` into the component's generated span records, just as `leaf.changed` is mapped. |
| Native and guest SDK adapters | Continue using the shared record/tree conversions above; no separate highlight payload. |

Mapping a span copies coordinates exactly: no line renumbering, merging with
`changed`, or copying highlights from one side to the other. Leaf constructors
for ordinary diff output initialize an empty vector. Rebuild generated bindings
and plugin components against the updated WIT before using this field.

### What happens when a plugin splits a leaf

A leaf covers source lines. `Cut` divides it into two leaves at a line boundary;
it does not edit the source text. Each highlight belongs to the resulting leaf
that contains its source line.

For example, using zero-based, half-open source ranges:

```text
Before: leaf [10, 16)
  search_highlights = [(line 11, columns 2..7), (line 14, columns 0..5)]

Cut at offset 3: the boundary is source line 13.

After:  leaf [10, 13)
  search_highlights = [(line 11, columns 2..7)]
        leaf [13, 16)
  search_highlights = [(line 14, columns 0..5)]
```

The spans keep their original source coordinates. A highlight on boundary line
13 belongs to the second leaf. Because a `Span` covers only one line and `Cut`
only cuts between lines, a span itself never needs to be cut. The combined
highlight coverage before and after the operation must be identical.

The applier already does this for `changed`. Extend `split` in
`crates/diffr-plugin-sdk/src/apply.rs` to read both vectors and construct each
piece with the same line-membership filter for each vector:

```rust
let Node::Leaf { changed, search_highlights, .. } = &leaf.node else {
    unreachable!("only leaves are cut");
};

// Inside the existing piece constructor; `lines` is this piece's source range.
Node::Leaf {
    alignment_id,
    changed: changed.iter().copied()
        .filter(|span| lines.contains(&span.line)).collect(),
    search_highlights: search_highlights.iter().copied()
        .filter(|span| lines.contains(&span.line)).collect(),
}
```

When a leaf has an aligned counterpart, `Cut` also splits that counterpart at the
same relative line offset. Each side distributes only its own spans, using its
own source line numbers. An unmatched counterpart stays unhighlighted.

`JoinFolds` retains the child leaves, so their highlight data stays in place.
`LinkFoldState`, `SetCollapsed`, `SetLabel`, and `SetTags` do not reconstruct leaf
span data. Linking and collapsing can still hide highlights; preserving coverage
and keeping matches visible are separate requirements for plugin behavior.

## Jev selection adapter

The optional `diffr/jev` adapter uses the official `@typesafe-ai/sdk` in
TypeScript. Insert it between hydration and postprocessing:

```typescript
import * as diffr from "diffr/api";
import { createJevRanker } from "diffr/jev";
import { TypeSafeClient } from "@typesafe-ai/sdk";

const jev = createJevRanker({ client: new TypeSafeClient(), concurrency: 4 });
const hydrated = await diffr.hydrate(scope, hits);
const ranked = await jev.rank("Find the code that describes the retry label", hydrated);
console.table(ranked.map(({ result, side, region, score }) => ({
  file: result.file[side]!.path, side, line: region.start.line + 1, score,
})));
const selected = jev.filter(ranked, { minScore: 0.6, limit: 5 });
const results = await diffr.postprocess(scope, selected);
console.log(results);
```

`rank` judges each candidate region independently, including its side, relative
path, source lines, change markers, and search-highlight lines. It includes three
preceding lines as background because a body fold can exclude its signature.
It does not send absolute worktree paths or the entire file. Duplicate hydrated
candidates remain separate, as they do before other selection strategies.

Each score is a Noul: the probability that the candidate satisfies the query,
not a degree-of-relevance score or a separate confidence value. Results sort
highest first with stable ties; the caller can inspect each candidate's `model`
and token `usage`. The model inherits the TypeSafe client's default unless
`createJevRanker({ model })` overrides it. `rank` accepts SDK request options,
including an abort signal, as its third argument. Errors reject the operation;
there is no fallback that silently selects candidates after a failed judgment.

`filter` is local and makes no model calls. `minScore` is inclusive; `limit` is a
global candidate-region limit across both sides and all files. The example's
0.6 is an illustrative threshold, not a calibrated guarantee. Selected regions
retain their original file pairing and side. The returned results are independent
copies with the usual methods, ready for `postprocess`; neither operation mutates
the hydrated inputs. Postprocessing restores display order and still shows diff
changes, including changes outside the selected search regions.

Run `bun run test:jev` for the live fixture example using `TYPESAFE_API_KEY`
(Bun loads `.env`). It prints every score and the selected pretty-printed output.
The regular code-mode suite tests the SDK transport contract deterministically;
the live example uses the real service and fails if credentials are missing.

## Composition example

The eval host can preload the imports. `scope` below is a caller-supplied plain
object; Rust creates or reuses the comparison index for its pins.

```js
import * as diffr from "diffr/api";
import { execFile } from "node:child_process";
import { promisify } from "node:util";
import * as fs from "node:fs/promises";

const exec = promisify(execFile);
const scope = {
  repo: "/repo",
  baseWorktree: { commitId: baseCommitId, path: "/checkouts/base" },
  headWorktree: { commitId: headCommitId, path: "/checkouts/head" },
};

// 1. RETRIEVE: normal rg across both worktrees; no diffr search API.
const { stdout } = await exec("rg", [
  "--json", "--fixed-strings", "--", "apply_fold",
  scope.baseWorktree.path, scope.headWorktree.path,
], { maxBuffer: 16 * 1024 * 1024 }).catch(error => {
  if (error.code === 1) return { stdout: "" }; // No matches.
  throw error;
});

// 2. ADAPT: ordinary JS, here for rg's default single-line text mode.
const hits = stdout.split("\n").filter(Boolean)
  .map(line => JSON.parse(line))
  .filter(event => event.type === "match")
  .map(({ data }) => {
    if (data.path.text === undefined || data.lines.text === undefined) {
      throw new Error("This adapter requires text paths and content");
    }
    return {
      file: data.path.text,
      lines: [{
        line: data.line_number,
        text: data.lines.text.replace(/\r?\n$/, ""),
      }],
    };
  });

// Optional: inspect raw hits using their paired line/text representation.
console.log(hits.flatMap(hit =>
  hit.lines.map(({ line, text }) => `${hit.file}:${line}: ${text}`)
).join("\n"));

// 3. HYDRATE: attach query highlights to the indexed search-region trees.
const hydrated = await diffr.hydrate(scope, hits);

// 4. SELECT: rank candidate regions within each file's base/head sources.
// Keep up to five matching candidates per side, preferring changed matches.
for (const result of hydrated) {
  for (const source of Object.values(result.sources)) {
    source.regions = source.regions
      .filter(region => region.hasHighlights())
      .sort((a, b) => Number(b.hasChangedHighlights()) - Number(a.hasChangedHighlights()))
      .slice(0, 5);
  }
}

// SearchResult carries the selected regions together with their file pairing.
const selected = hydrated.filter(result =>
  Object.values(result.sources).some(source => source.regions.length > 0)
);

// 5. POSTPROCESS: deduplicate, coalesce regions, and apply context/folding plugins.
const results = await diffr.postprocess(scope, selected);

// 6. READ: each object pretty-prints like numbered source plus a unified diff.
console.log(results[0]);
console.log(results);

// Structured data is still available for normal JS and persistence.
await fs.writeFile("/scratch/results.json", JSON.stringify(results));

// An eval host that displays completion values through Node inspection can
// display these directly. A JSON-serializing host receives toJSON() data instead.
results;
```

Illustrative custom-inspection output for one result:

```text
src/retry.ts — base → head
 base  head
              … base 1–17 / head 1–17 collapsed [fold_state_id=4] …
   18    18    async function retry(request) {
   19       -   const attempts = 3;
         19 +   const attempts = options.attempts ?? 3;
   20    20      for (let i = 0; i < attempts; i++) {
              … base 21–28 / head 21–28 collapsed [fold_state_id=12] …
   29    29      throw lastError;
   30    30    }
              … base 31–64 / head 31–65 collapsed [fold_state_id=19] …

[More context: call result.setCollapsed(fold_state_id, false) with an indicated
ID, then print the result again. Full source
text and region children are already present; no read call is needed.]
```

Display line numbers are 1-based. Unchanged evidence can print as a single numbered
source excerpt. Gaps remain explicit and source line numbers are never renumbered.
Every collapsed area, including leading/trailing content, prints its side-specific
line ranges and a file-local `fold_state_id`. The helper updates matching regions
on both included sides, since that ID links their collapse state. Missing sides have no
range. These notices describe folded content, not output-budget truncation.

For example, ordinary JS can open the middle gap above:

```js
const result = results[0];

// Open the linked regions on both included sides.
result.setCollapsed(12, false);
console.log(result);

```

The printer uses current visibility each time; it must not reuse a stale rendered
string. Opening an outer fold can reveal further collapsed children with their own
hints. No need to rerun postprocessing merely to expand a fold, since plugins could
collapse it again. Visibility edits are local to this result, even when source text
is shared with other results.

Like [Pi's read tool](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/src/core/tools/read.ts),
omission notices give an actionable next step. Here that step edits the in-memory
object rather than fetching another page.

Saving JSON preserves data, not methods; a future deserialization helper can restore
object printing without recomputing the comparison.

Future Review composition (illustrative, not part of this API):

```js
review.anchor(results[0]);
review.codePeek(results[0]);
// Use the same result as a sequence step's source evidence.
```

The leaf highlights identify matched lines; the region trees supply expanded
context and visibility. A future Review adapter can consume these together without
confusing matched lines with surrounding context. The extended leaf field and
client envelope are not asserted to be accepted by the current Review API unchanged.
