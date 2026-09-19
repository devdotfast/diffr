import { afterAll, beforeAll, expect, test } from "bun:test";
import * as diffr from "diffr/api";
import { searchResultDataSchema } from "diffr/schema";
import { execFileSync } from "node:child_process";
import { createFixture, grep } from "./fixture";

let fixture: Awaited<ReturnType<typeof createFixture>>;
let hits: diffr.Hit[];
beforeAll(async () => {
  fixture = await createFixture();
  hits = await grep(fixture.scope, "search_token");
});
afterAll(async () => { await fixture?.cleanup(); });

test("reject stale text, invalid lines, paths outside the scope, and wrong pins", async () => {
  const { scope } = fixture;
  const hit = hits[0];
  await expect(diffr.hydrate(scope, [{ ...hit, lines: [{ line: 1, text: "stale" }] }]))
    .rejects.toThrow("hit text differs");
  await expect(diffr.hydrate(scope, [{ ...hit, lines: [{ line: 0, text: "" }] }]))
    .rejects.toThrow("1-based");
  await expect(diffr.hydrate(scope, [{ ...hit, lines: [{ line: 999, text: "" }] }]))
    .rejects.toThrow("out of bounds");
  await expect(diffr.hydrate(scope, [{ ...hit, file: import.meta.path }]))
    .rejects.toThrow("outside scoped worktrees");
  await expect(diffr.hydrate({ ...scope, headWorktree: {
    ...scope.headWorktree, commitId: scope.baseWorktree.commitId,
  } }, [])).rejects.toThrow("worktree HEAD");
});

test("selection coalesces repeated candidates without highlighting the counterpart", async () => {
  const { scope } = fixture;
  const hit = hits.find(hit => hit.file.endsWith("head/retry.js") && hit.lines[0].line === 3)!;
  const hydrated = await diffr.hydrate(scope, [hit, hit]);
  const selected = hydrated[0];
  expect(selected.sources.rhs!.regions.some(region => region.hasHighlights())).toBe(true);
  expect(selected.sources.rhs!.regions.some(region => region.hasChangedHighlights())).toBe(false);
  const [result] = await diffr.postprocess(scope, hydrated);
  const spans = (source: diffr.Source) => {
    const out: diffr.Span[] = [];
    function visit(regions: diffr.Region[]) {
      for (const region of regions) {
        if (region.kind === "fold") visit(region.children);
        else out.push(...region.search_highlights ?? []);
      }
    }
    visit(source.regions);
    return out;
  };
  expect(spans(result.sources.lhs!)).toEqual([]);
  expect(spans(result.sources.rhs!)).toEqual([{
    line: 2, start_column: 0, end_column: Buffer.byteLength(hit.lines[0].text),
  }]);
});

test("expanding a fold updates printing locally without rerunning plugins", async () => {
  const { scope } = fixture;
  const hydrated = await diffr.hydrate(scope, hits);
  const [first, second] = await Promise.all([
    diffr.postprocess(scope, hydrated), diffr.postprocess(scope, hydrated),
  ]);
  const result = first.find(result => result.file.rhs?.path === "retry.js")!;
  const other = second.find(result => result.file.rhs?.path === "retry.js")!;
  const before = other.toString();
  const id = Number(result.toString().match(/fold_state_id=(\d+)/)![1]);
  function flatten(regions: diffr.Region[]): diffr.Region[] {
    return regions.flatMap(region => [region,
      ...(region.kind === "fold" ? flatten(region.children) : [])]);
  }
  const outer = flatten(result.sources.rhs!.regions).find(region => region.fold_state_id === id)!;
  if (outer.kind !== "fold") throw new Error("Expected the try body fold");
  const child = flatten(outer.children).find(region => region.fold_state_id !== id)!;
  // Explicitly collapse a child: context need not create a collapsed child.
  result.setCollapsed(child.fold_state_id, true);
  result.setCollapsed(id, false);
  expect(result.toString()).not.toContain(`fold_state_id=${id}]`);
  expect(result.toString()).toContain(`fold_state_id=${child.fold_state_id}]`);
  expect(result.toString()).not.toContain("return response;");
  result.setCollapsed(child.fold_state_id, false);
  expect(result.toString()).toContain("return response;");
  expect(result.toString()).toContain("const response = request();");
  expect(other.toString()).toBe(before);
  expect(() => result.setCollapsed(999999, false)).toThrow("No fold_state_id");
});

test("postprocessing accepts plugin settings and a deliberate side-only view", async () => {
  const { scope } = fixture;
  const [result] = await diffr.hydrate(scope, hits.filter(hit => hit.file.endsWith("head/retry.js")));
  result.display = "rhs";
  const [processed] = await diffr.postprocess(scope, [result], { plugins: { order: [] } });
  expect(processed.sources.lhs).toBeDefined();
  expect(processed.display).toBe("rhs");
  expect(processed.toString()).toContain("const response = request();");
  expect(processed.toString()).not.toContain("collapsed");
});

test("Git rename correspondence pairs different base and head paths", async () => {
  const renamed = await createFixture();
  try {
    const { scope } = renamed;
    const git = (...args: string[]) => execFileSync("git", [
      "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid",
      "-c", "commit.gpgSign=false", "-c", "core.hooksPath=/dev/null", ...args,
    ], { cwd: scope.repo, encoding: "utf8" }).trim();
    git("mv", "retry.js", "renamed.js");
    git("commit", "--quiet", "-m", "Rename fixture");
    scope.headWorktree.commitId = git("rev-parse", "HEAD");
    git("-C", scope.headWorktree.path, "checkout", "--quiet", "--detach", scope.headWorktree.commitId);
    const results = await diffr.hydrate(scope, await grep(scope, "search_token"));
    const result = results.find(result => result.file.rhs?.path === "renamed.js")!;
    expect(result.file.lhs?.path).toBe("retry.js");
    expect(result.display).toBe("both");
  } finally { await renamed.cleanup(); }
});


test("wire schema accepts real hydrated and processed results and rejects corrupt evidence", async () => {
  const hydrated = await diffr.hydrate(fixture.scope, hits);
  const processed = await diffr.postprocess(fixture.scope, hydrated);
  for (const result of [...hydrated, ...processed]) {
    const wire = JSON.parse(JSON.stringify(result));
    expect(searchResultDataSchema.parse(wire)).toEqual(wire);
  }
  const wire = JSON.parse(JSON.stringify(processed.find(result => result.file.rhs?.path === "retry.js")));
  delete wire.file.lhs;
  expect(searchResultDataSchema.safeParse(wire).success).toBe(false);
  wire.file = {rhs: processed.find(result => result.file.rhs?.path === "retry.js")!.file.rhs};
  wire.sources = {rhs: wire.sources.rhs};
  wire.display = "rhs";
  expect(searchResultDataSchema.safeParse(wire).success).toBe(true);
  wire.sources.rhs.regions[0].end.line = 999999;
  expect(searchResultDataSchema.safeParse(wire).success).toBe(false);
});


test("shared unchanged trees survive hydration, processing and every display choice", async () => {
  const [result] = await diffr.hydrate(fixture.scope, hits.filter(hit => hit.file.endsWith("/unchanged.js")));
  expect(result.sources.same).toBeDefined();
  expect(result.sources.lhs).toBeUndefined();
  expect(result.sources.rhs).toBeUndefined();
  expect(result.file.lhs).toBeDefined();
  expect(result.file.rhs).toBeDefined();
  const [processed] = await diffr.postprocess(fixture.scope, [result]);
  expect(processed.sources.same).toBeDefined();
  for (const display of ["both", "lhs", "rhs"] as const) {
    processed.display = display;
    expect(searchResultDataSchema.safeParse(processed.toJSON()).success).toBe(true);
    expect(processed.toString()).toContain("search_token: wholly unchanged file");
  }
});

test("postprocessing rejects truncated comparisons and rewritten change classification", async () => {
  const [result] = await diffr.hydrate(fixture.scope, hits.filter(hit => hit.file.endsWith("head/retry.js")));
  const original = result.sources.rhs!.regions;
  result.sources.rhs!.regions = [];
  await expect(diffr.postprocess(fixture.scope, [result])).rejects.toThrow("differs from index");
  result.sources.rhs!.regions = original;
  const walk = (regions: diffr.Region[]) => {
    for (const region of regions) {
      if (region.kind === "fold") walk(region.children);
      else region.changed = [];
    }
  };
  walk(original);
  await expect(diffr.postprocess(fixture.scope, [result])).rejects.toThrow("differs from index");
});
