// bun tests/code-mode/review.live.ts REVIEW_REPO BASE HEAD DISCOVERY_JSON OUTPUT_DIR
// Run against an isolated Review dev instance. Creates one review and leaves it open.
import * as diffr from "diffr/api";
import { searchResultDataSchema } from "diffr/schema";
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";

const [repository, base, head, discoveryFile, outputDirectory] = process.argv.slice(2);
if (!outputDirectory) throw new Error("Expected REVIEW_REPO BASE HEAD DISCOVERY_JSON OUTPUT_DIR");
const repo = resolve(repository);
const output = resolve(outputDirectory);
await mkdir(output, {recursive: true});
const git = (...args: string[]) => execFileSync("git", args, {cwd: repo, encoding: "utf8"}).trim();
const temporary = await mkdtemp(join(tmpdir(), "review-diffr-e2e-"));
const scope: diffr.Scope = {repo,
  baseWorktree: {path: join(temporary, "base"), commitId: git("rev-parse", base)},
  headWorktree: {path: join(temporary, "head"), commitId: git("rev-parse", head)},
};
const discovery = JSON.parse(await readFile(discoveryFile, "utf8"));
async function request(route: string, body?: unknown) {
  const response = await fetch(`${discovery.url}/reviews-api${route}`, {
    method: body === undefined ? "GET" : "POST",
    headers: {"x-review-token": discovery.token, "content-type": "application/json"},
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const result = await response.json();
  if (!response.ok) throw new Error(`${response.status}: ${JSON.stringify(result)}`);
  return result;
}
const command = (operation: unknown) => request("/commands", {commandId: crypto.randomUUID(), operation});
try {
  for (const tree of [scope.baseWorktree, scope.headWorktree]) git("worktree", "add", "--quiet", "--detach", tree.path, tree.commitId);
  const file = join(scope.headWorktree.path, "packages/review/src/review-api/local-data.ts");
  const events = execFileSync("rg", ["--no-config", "--json", "--fixed-strings", "Diffr evidence text differs", file], {encoding: "utf8"});
  const hits: diffr.Hit[] = events.split("\n").filter(Boolean).map(line => JSON.parse(line)).filter(event => event.type === "match").map(({data}) => ({file: data.path.text, lines: [{line: data.line_number, text: data.lines.text.replace(/\r?\n$/, "")}]}));
  assert.ok(hits.length);
  const hydrated = await diffr.hydrate(scope, hits);
  const [paired] = await diffr.postprocess(scope, hydrated);
  const selected = (await diffr.hydrate(scope, hits))[0];
  selected.display = "rhs";
  const [rhs] = await diffr.postprocess(scope, [selected]);
  assert.ok(paired.sources.lhs && paired.sources.rhs);
  assert.ok(rhs.sources.lhs);
  assert.equal(rhs.display, "rhs");
  const unchangedFile = join(scope.headWorktree.path, "AGENTS.md");
  const unchangedText = (await readFile(unchangedFile, "utf8")).split("\n")[0];
  const [unchanged] = await diffr.postprocess(scope, await diffr.hydrate(scope, [{file: unchangedFile, lines: [{line: 1, text: unchangedText}]}]));
  assert.ok(unchanged.sources.same);
  const baseFile = join(scope.baseWorktree.path, "packages/review/src/review-api/local-data.ts");
  const baseLines = (await readFile(baseFile, "utf8")).split("\n");
  const line = baseLines.findIndex(line => line.includes("async validateSource("));
  assert.ok(line >= 0);
  const [left] = await diffr.hydrate(scope, [{file: baseFile, lines: [{line: line + 1, text: baseLines[line]}]}]);
  left.display = "lhs";
  const [lhs] = await diffr.postprocess(scope, [left]);
  const results = [rhs, paired, unchanged, lhs].map(result => searchResultDataSchema.parse(result.toJSON()));
  await writeFile(join(output, "evidence.json"), JSON.stringify(results, null, 2));
  await writeFile(join(output, "pretty.txt"), [rhs, paired, unchanged, lhs].map(result => result.toString()).join("\n\n"));
  console.log(`${hits.length} raw hits\n\n${rhs.toString()}`);
  const repositoryRecord = await request("/repositories", {path: repo});
  const pins = await request("/pins", {repositoryId: repositoryRecord.id, base: scope.baseWorktree.commitId, head: scope.headWorktree.commitId});
  const {reviewId} = await command({type: "create", title: "Diffr evidence: implementation self-review", pins});
  const insert = (content: unknown) => command({type: "edit", reviewId, edit: {type: "insert", content}});
  await insert({type: "markdown", markdown: "# Direct search evidence\n\nThis result came from a real diffr query over this implementation. The first excerpt deliberately contains only the head side of a modified file. Expand its folds, search within it, and open the highlighted source line."});
  await insert({type: "code_peek", source: results[0]});
  await insert({type: "file_lens", title: "Head-only evidence", targets: [{kind: "results", results: [results[0]]}]});
  await insert({type: "file_lens", title: "Paired evidence", targets: [{kind: "results", results: [results[1]]}]});
  await insert({type: "file_lens", title: "Mixed files and evidence", targets: [{kind: "files", patterns: ["packages/review/src/source.ts"]}, {kind: "results", results: [results[0], results[2], results[3]]}]});
  await insert({type: "code_peek", source: results[2]});
  await insert({type: "sequence", title: "Evidence validation", actors: {model: "Model", review: "Review"}, steps: [{from: "model", to: "review", label: "Submit head-only evidence", source: results[0]}, {from: "review", to: "model", label: "Inspect paired evidence", source: results[1]}]});
  // Remove query worktrees before reopening: Review must use saved text and pinned Git identity.
  for (const tree of [scope.baseWorktree, scope.headWorktree]) git("worktree", "remove", "--force", tree.path);
  const saved = await request(`/${reviewId}?full=true`);
  const peek = saved.document.find((block: {type: string}) => block.type === "code_peek");
  assert.deepEqual(peek.source, results[0]);
  const progress = await request(`/${reviewId}/progress`);
  const mixed = progress.diagrams.find((lens: {title: string}) => lens.title === "Mixed files and evidence");
  assert.ok(mixed.targets[0].ranges.some((range: {file: string}) => range.file === "packages/review/src/source.ts"));
  assert.deepEqual(mixed.targets[1].results, [results[0], results[2], results[3]]);
  await request(`/${reviewId}/open`, {});
  await writeFile(join(output, "review.json"), JSON.stringify({reviewId, version: saved.version, pins}, null, 2));
  console.log(`Opened ${reviewId}, version ${saved.version}; query worktrees removed.`);
} finally {
  for (const tree of [scope.baseWorktree, scope.headWorktree]) {
    try {if (existsSync(tree.path)) git("worktree", "remove", "--force", tree.path);} catch { /* already removed */ }
  }
  await rm(temporary, {recursive: true, force: true});
}
