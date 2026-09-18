import { afterAll, beforeAll, expect, test } from "bun:test";
import * as diffr from "diffr/api";
import { createJevRanker, filter } from "diffr/jev";
import { TypeSafeClient } from "@typesafe-ai/sdk";
import { createFixture, grep } from "./fixture";

let fixture: Awaited<ReturnType<typeof createFixture>>;
let hydrated: diffr.SearchResult[];
beforeAll(async () => {
  fixture = await createFixture();
  hydrated = await diffr.hydrate(fixture.scope, await grep(fixture.scope, "search_token"));
});
afterAll(async () => { await fixture?.cleanup(); });

test("SDK transport scores every region; filtering preserves sides and independent results", async () => {
  // Deterministic transport contract test, separate from the real Jev run.
  let calls = 0;
  let active = 0;
  let peak = 0;
  const client = new TypeSafeClient({ apiKey: "test-only", fetch: async (_url, init) => {
    calls++;
    peak = Math.max(peak, ++active);
    await new Promise(resolve => setTimeout(resolve, 1));
    const { state } = JSON.parse(String(init!.body));
    expect(state.query).toBe("Explain the retry label on the head side");
    expect(state.candidate.lines.some((line: { matched: boolean }) => line.matched)).toBe(true);
    const relevant = state.candidate.path === "retry.js" && state.candidate.lines[0].line === 21;
    if (relevant) expect(state.candidate.contextBefore.join("\n")).toContain("function describeRetry()");
    active--;
    return Response.json({ model: "jev-test", answers: { relevant: { type: "noul",
      noul: relevant ? (state.candidate.revision === "head" ? 0.9 : 0.7) : 0.1,
    } }, usage: { input_tokens: 10, output_tokens: 1 } });
  } });
  const before = JSON.stringify(hydrated);
  const jev = createJevRanker({ client, concurrency: 2 });
  const ranked = await jev.rank("Explain the retry label on the head side", hydrated);
  const count = hydrated.reduce((n, result) => n + Object.values(result.sources)
    .reduce((m, source) => m + source.regions.length, 0), 0);
  expect(calls).toBe(count);
  expect(peak).toBe(2);
  expect(ranked[0].side).toBe("rhs");
  expect(ranked[0].score).toBe(0.9);
  const selected = jev.filter(ranked, { minScore: 0.9, limit: 1 });
  expect(selected).toHaveLength(1);
  expect(selected[0].sources.lhs!.regions).toHaveLength(0);
  expect(selected[0].sources.rhs!.regions).toHaveLength(1);
  expect(selected[0].sources.rhs!.regions[0].hasHighlights()).toBe(true);
  const processed = await diffr.postprocess(fixture.scope, selected);
  expect(processed[0].toString()).toContain("function describeRetry()");
  selected[0].setCollapsed(selected[0].sources.rhs!.regions[0].fold_state_id, false);
  expect(JSON.stringify(hydrated)).toBe(before);
  expect(jev.filter(ranked, { minScore: 1 })).toEqual([]);
  expect(jev.filter(ranked, { minScore: 0, limit: 0 })).toEqual([]);
  expect(calls).toBe(count); // Threshold changes never call Jev.
});

test("empty input, invalid options, cancellation, and service errors do not silently select", async () => {
  expect(await createJevRanker().rank("anything", [])).toEqual([]);
  expect(() => createJevRanker({ concurrency: 0 })).toThrow("concurrency");
  expect(() => filter([], { minScore: NaN })).toThrow("minScore");
  expect(() => filter([], { minScore: 0, limit: -1 })).toThrow("limit");
  const client = new TypeSafeClient({ apiKey: "test-only", retry: { maxRetries: 0 },
    fetch: async () => new Response("unavailable", { status: 503 }) });
  const jev = createJevRanker({ client, concurrency: 1 });
  await expect(jev.rank(" ", hydrated)).rejects.toThrow("query");
  await expect(jev.rank("retry", hydrated)).rejects.toThrow();
  await expect(jev.rank("retry", hydrated, { signal: AbortSignal.abort() })).rejects.toThrow();
});
