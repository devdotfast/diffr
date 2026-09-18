# Code-mode output test

Review [search.expected.txt](./search.expected.txt) first. It is the proposed
plain-text rendering checked against the real Rust search engine and plugins. Changed files come first, then wholly unchanged files.
Within each category files are sorted by path. Fold IDs are normalized to `<id>`
for comparison; actual printed results will contain numeric IDs.

[search.test.ts](./search.test.ts) imports `diffr/api` at the top level, searches
both fixture revisions, hydrates every hit, runs postprocessing, prints the
results, and compares that output against the file. There are no skips, mocks,
module overrides, result filters, or assertions over internal tree details.

```sh
bun install
bun run build:api
bun run typecheck
bun run test:code-mode
```

The Node-API addon calls the shared Rust engine. Hydration validates hits against
pinned Git blobs and caches structural analysis; postprocessing runs the bundled
plugins before the JS printer renders the results. The output test passes without
mocking these steps.

The context plugin keeps both boundaries of visible scopes open. In `retry.js`,
lines 7–12 collapse while the `try`/`catch` and `for` closing braces remain
visible. The output comparison checks these boundaries as well as every hit.

`fixture.ts` creates and cleans up a temporary Git repository and two pinned
worktrees. The ten matched lines include adjacent changed and unchanged hits
inside `retry()`, an unchanged function in `retry.js`, a wholly unchanged file,
a deleted file, and an added file. The unchanged hit inside `retry()` prints
as a context row, without a `+` or `-`, beside the changed configuration rows.
Unchanged evidence found on both sides counts twice in the raw hit total but
can be rendered once in the combined result. No source checkout is modified.
