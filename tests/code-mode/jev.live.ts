// Opt-in live integration: TYPESAFE_API_KEY is required; no mocks or fallbacks.
import * as diffr from "diffr/api";
import { createJevRanker } from "diffr/jev";
import { createFixture, grep } from "./fixture";
import { strict as assert } from "node:assert";

const fixture = await createFixture();
try {
  const hits = await grep(fixture.scope, "search_token");
  const hydrated = await diffr.hydrate(fixture.scope, hits);
  const jev = createJevRanker();
  const query = "Find the function that describes the retry label without performing retries.";
  const ranked = await jev.rank(query, hydrated);
  console.log(`Query: ${query}\nTotal matched lines: ${hits.reduce((n, hit) => n + hit.lines.length, 0)}`);
  for (const { result, side, region, score, model } of ranked) {
    console.log(`${score.toFixed(4)}  ${result.file[side]!.path}:${region.start.line + 1}–${region.end.line} (${side}, ${model})`);
  }
  assert.equal(ranked[0].result.file[ranked[0].side]!.path, "retry.js");
  assert.equal(ranked[0].region.start.line, 20, "describeRetry should rank first");
  const selected = jev.filter(ranked, { minScore: 0.6 });
  assert.ok(selected.length > 0, "Jev must select relevant evidence");
  const results = await diffr.postprocess(fixture.scope, selected);
  console.log("\n" + results.map(result => result.toString()).join("\n\n"));
  assert.ok(results.some(result => result.toString().includes("export function describeRetry()")));
} finally {
  await fixture.cleanup();
}
