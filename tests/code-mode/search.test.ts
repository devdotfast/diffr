import { afterAll, beforeAll, expect, test } from "bun:test";
import * as diffr from "diffr/api";
import { readFile } from "node:fs/promises";
import { createFixture, grep } from "./fixture";

let fixture: Awaited<ReturnType<typeof createFixture>>;
beforeAll(async () => { fixture = await createFixture(); });
afterAll(async () => { await fixture?.cleanup(); });

test("pretty-print every search hit, changed files first", async () => {
  const { scope } = fixture;
  const hits = await grep(scope, "search_token");
  const results = await diffr.postprocess(scope, await diffr.hydrate(scope, hits));
  const path = (result: diffr.SearchResult) => (result.file.rhs ?? result.file.lhs).path;
  results.sort((a, b) =>
    Number(a.kind === "unchanged") - Number(b.kind === "unchanged") ||
    path(a).localeCompare(path(b))
  );
  const output = [
    `Total matched lines: ${hits.reduce((n, hit) => n + hit.lines.length, 0)}`,
    ...results.map(result => result.toString()),
  ].join("\n\n");
  console.log(output);
  const expected = await readFile(new URL("./search.expected.txt", import.meta.url), "utf8");
  // Fold IDs are allocated internally; compare their presence, not their numbering.
  expect(output.replace(/fold_state_id=\d+/g, "fold_state_id=<id>").trimEnd())
    .toBe(expected.trimEnd());
});
