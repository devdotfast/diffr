import { expect, test } from "bun:test";
import { readDiffStream } from "./stream";
import { createTestDiffFile, fold, leaf } from "./fixture";
import type { FileChange } from "./wire";
const manifest = (file: ReturnType<typeof createTestDiffFile>): FileChange =>
  ({ file: file.file, status: "modified", visibility: { collapsed: false, label: "" } });
const start = {
  type: "start",
  version: 2,
  lhs: { type: "index" },
  rhs: { type: "working_tree" },
  files: [manifest(createTestDiffFile())],
};
async function decode(events: unknown[]) {
  const text = events.map((e) => JSON.stringify(e)).join("\n"),
    bytes = new TextEncoder().encode(text);
  async function* chunks() {
    for (const byte of bytes) yield Uint8Array.of(byte);
  }
  return Array.fromAsync(readDiffStream(chunks()));
}
test("decode byte-fragmented Unicode stream and preserve region trees", async () => {
  const file = createTestDiffFile();
  file.file = { lhs: { ...file.file.lhs!, path: "变量.ts" }, rhs: { ...file.file.rhs!, path: "变量.ts" } };
  if (file.diff.type !== "text") throw new Error();
  file.diff.lhs!.regions = [fold(9, [0, 0], [2, 8], [leaf(1, 0, 3)], "Body", ["function"], true)];
  const events = [{...start, files: [manifest(file)]}, file, { type: "complete", succeeded: 1, failed: 0 }];
  expect((await decode(events)) as unknown).toEqual(events);
});
test("omitted defaults are filled in", async () => {
  const file = createTestDiffFile();
  const bare = { type: "file", file: file.file, diff: { type: "text",
    lhs: { text: "a\n", regions: [{ id: 1, kind: "leaf", start: { line: 0, column: 0 }, end: { line: 1, column: 0 } }] },
    rhs: { text: "a\n", regions: [{ id: 1, kind: "leaf", start: { line: 0, column: 0 }, end: { line: 1, column: 0 } }] },
    stats: { textual: { added: 0, removed: 0 }, visible: { added: 0, removed: 0 } } } };
  const [, decoded] = await decode([{ ...start, files: [{ file: file.file, status: "modified" }] }, bare,
    { type: "complete", succeeded: 1, failed: 0 }]);
  expect(decoded).toMatchObject({ diff: { lhs: { syntax: [], regions: [{ tags: [], changed: [], children: [],
    visibility: { collapsed: false, label: "" } }] } } });
});
test("reject missing completion, counts, unknown version and invalid ordering", async () => {
  for (const events of [
    [start],
    [{ ...start, version: 1 }],
    [createTestDiffFile()],
    [start, { type: "complete", succeeded: 1, failed: 0 }],
    [start, start],
    [start, { type: "file", file: createTestDiffFile().file }],
    [start, { ...createTestDiffFile(), error: { code: "x", message: "y" } }],
  ])
    await expect(decode(events)).rejects.toThrow();
});
test("file errors and aborts complete without discarding earlier successful results", async () => {
  const file = createTestDiffFile();
  const failed = { lhs: { path: "failed.ts", oid: "3", mode: "100644" }, rhs: { path: "failed.ts", oid: "4", mode: "100644" } };
  const decoded = await decode([
    { ...start, files: [manifest(file), { file: failed, status: "modified" }] },
    file,
    { type: "file", file: failed, error: { code: "not_utf8", message: "unreadable" } },
    { type: "complete", succeeded: 1, failed: 1 },
  ]);
  expect(decoded.length).toBe(4);
  const aborted = await decode([
    { ...start, files: [manifest(file), { file: failed, status: "modified" }] },
    file,
    { type: "complete", succeeded: 1, failed: 0, aborted: { code: "hook_failed", message: "503" } },
  ]);
  expect(aborted.length).toBe(3);
  await expect(decode([
    { ...start, files: [manifest(file), { file: failed, status: "modified" }] },
    file,
    { type: "complete", succeeded: 1, failed: 0 },
  ])).rejects.toThrow();
});
test("manifest validates identities while allowing results in arrival order", async () => {
  const a = createTestDiffFile(), b = createTestDiffFile();
  b.file = { lhs: { path: "b.ts", oid: "5", mode: "100644" }, rhs: { path: "b.ts", oid: "6", mode: "100644" } };
  for (const events of [
    [{...start, files: []}, { type: "complete", succeeded: 0, failed: 0 }, a],
    [{...start, files: [manifest(a), manifest(a)]}],
    [{...start, files: [manifest(a), manifest(b)]}, b, b],
    [start, b],
  ]) await expect(decode(events)).rejects.toThrow();
  expect((await decode([{...start, files: [manifest(a), manifest(b)]}, b, a,
    {type: "complete", succeeded: 2, failed: 0}])).map(e => e.type))
    .toEqual(["start", "file", "file", "complete"]);
});
