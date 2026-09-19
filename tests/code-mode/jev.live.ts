// Opt-in live integration: TYPESAFE_API_KEY is required; no mocks or fallbacks.
import * as diffr from "diffr/api";
import { createJevRanker } from "diffr/jev";
import { TypeSafeClient } from "@typesafe-ai/sdk";
import { createFixture, grep } from "./fixture";
import { strict as assert } from "node:assert";
import { writeFile } from "node:fs/promises";

const fixture = await createFixture();
// Optional first argument saves exact HTTP request/response bodies, never headers.
const tracePath = process.argv[2];
const exchanges: unknown[] = [];
try {
  const hits = await grep(fixture.scope, "search_token");
  const hydrated = await diffr.hydrate(fixture.scope, hits);
  const results = await diffr.postprocess(fixture.scope, hydrated);
  const client = new TypeSafeClient({ fetch: async (url, init) => {
    const response = await fetch(url, init);
    if (tracePath) exchanges.push({ request: JSON.parse(String(init!.body)),
      status: response.status, response: await response.clone().json() });
    return response;
  } });
  const jev = createJevRanker({ client });
  const query = "Find code that describes a retry label without performing retries.";
  const ranked = await jev.rank(query, results);
  console.log(`Query: ${query}\nTotal matched lines: ${hits.reduce((n, hit) => n + hit.lines.length, 0)}`);
  for (const { result, score, model } of ranked) {
    console.log(`${score.toFixed(4)}  ${(result.file.rhs ?? result.file.lhs)!.path} (${model})`);
  }
  const selected = jev.filter(ranked, { minScore: 0.6 });
  assert.ok(selected.some(result => result.file.rhs?.path === "retry.js"), "Select the retry description");
  assert.ok(!selected.some(result => result.file.rhs?.path === "unchanged.js"), "Exclude the cache description");
  console.log("\n" + selected.map(result => result.toString()).join("\n\n"));
} finally {
  if (tracePath) await writeFile(tracePath, JSON.stringify(exchanges, null, 2) + "\n");
  await fixture.cleanup();
}
