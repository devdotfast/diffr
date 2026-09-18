# Code-mode output test

Review [search.expected.txt](./search.expected.txt) first. It is the proposed
plain-text rendering, authored before implementation—not output captured from a
working search engine. Changed files come first, then wholly unchanged files.
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

The Node-API Rust addon compiles and loads. Its search entry points currently
reject with explicit not-implemented errors, so the integration test is **red**.
Indexing, search, plugin updates, and result printing follow after review.

`fixture.ts` creates and cleans up a temporary Git repository and two pinned
worktrees. The ten matched lines include adjacent changed and unchanged hits
inside `retry()`, an unchanged function in `retry.js`, a wholly unchanged file,
a deleted file, and an added file. The unchanged hit inside `retry()` prints
as a context row, without a `+` or `-`, beside the changed configuration rows.
Unchanged evidence found on both sides counts twice in the raw hit total but
can be rendered once in the combined result. No source checkout is modified.
