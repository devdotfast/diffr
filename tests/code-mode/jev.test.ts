import { afterAll, beforeAll, expect, test } from "bun:test";
import * as diffr from "diffr/api";
import { createJevRanker, filter } from "diffr/jev";
import { TypeSafeClient } from "@typesafe-ai/sdk";
import { createFixture, grep } from "./fixture";

let fixture: Awaited<ReturnType<typeof createFixture>>;
let results: diffr.SearchResult[];
beforeAll(async () => {
  fixture = await createFixture();
  const hydrated = await diffr.hydrate(fixture.scope, await grep(fixture.scope, "search_token"));
  results = await diffr.postprocess(fixture.scope, hydrated);
});
afterAll(async () => { await fixture?.cleanup(); });

test("SDK receives the exact printed result; filtering preserves its paired context", async () => {
  // Deterministic transport contract test, separate from the real Jev run.
  let calls = 0;
  let active = 0;
  let peak = 0;
  const bodies: string[] = [];
  const client = new TypeSafeClient({ apiKey: "test-only", fetch: async (_url, init) => {
    calls++;
    peak = Math.max(peak, ++active);
    await new Promise(resolve => setTimeout(resolve, 1));
    const { state } = JSON.parse(String(init!.body));
    expect(state.query).toBe("Explain the retry label");
    const original = results.find(result => result.toString() === state.result)!;
    expect(state).toEqual({ query: "Explain the retry label", result: original.toString() });
    bodies.push(state.result);
    active--;
    return Response.json({ model: "jev-test", answers: { relevant: { type: "noul",
      noul: original.file.rhs?.path === "retry.js" ? 0.9 : 0.1,
    } }, usage: { input_tokens: 10, output_tokens: 1 } });
  } });
  const before = JSON.stringify(results);
  const jev = createJevRanker({ client, concurrency: 2 });
  const ranked = await jev.rank("Explain the retry label", results);
  expect(calls).toBe(results.length);
  expect(bodies.sort()).toEqual(results.map(result => result.toString()).sort());
  expect(peak).toBe(2);
  expect(ranked[0].score).toBe(0.9);
  const selected = jev.filter(ranked, { minScore: 0.9, limit: 1 });
  expect(selected).toHaveLength(1);
  expect(selected[0].toJSON()).toEqual(ranked[0].result.toJSON());
  expect(selected[0].toString()).toBe(ranked[0].result.toString());
  expect(selected[0].toString()).toContain("function describeRetry()");
  expect(selected[0].toString()).toContain("collapsed");
  const foldId = Number(selected[0].toString().match(/fold_state_id=(\d+)/)![1]);
  selected[0].setCollapsed(foldId, false);
  expect(selected[0].toString()).not.toBe(ranked[0].result.toString());
  expect(JSON.stringify(results)).toBe(before);
  expect(jev.filter(ranked, { minScore: 1 })).toEqual([]);
  expect(jev.filter(ranked, { minScore: 0, limit: 0 })).toEqual([]);
  expect(calls).toBe(results.length);
});

test("empty input, invalid options, cancellation, and service errors do not silently select", async () => {
  expect(await createJevRanker().rank("anything", [])).toEqual([]);
  expect(() => createJevRanker({ concurrency: 0 })).toThrow("concurrency");
  expect(() => filter([], { minScore: NaN })).toThrow("minScore");
  expect(() => filter([], { minScore: 0, limit: -1 })).toThrow("limit");
  const client = new TypeSafeClient({ apiKey: "test-only", retry: { maxRetries: 0 },
    fetch: async () => new Response("unavailable", { status: 503 }) });
  const jev = createJevRanker({ client, concurrency: 1 });
  await expect(jev.rank(" ", results)).rejects.toThrow("query");
  await expect(jev.rank("retry", results)).rejects.toThrow();
  await expect(jev.rank("retry", results, { signal: AbortSignal.abort() })).rejects.toThrow();
});
