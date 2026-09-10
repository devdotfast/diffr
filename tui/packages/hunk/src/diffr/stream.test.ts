import { expect, test } from "bun:test";
import { readDiffStream } from "./stream";
import { createTestDiffFile } from "./fixture";
const start = {
  type: "start",
  version: 1,
  before: { kind: "index" },
  after: { kind: "working_tree" },
  total: 1,
  files: [createTestDiffFile().file],
};
async function decode(events: unknown[]) {
  const text = events.map((e) => JSON.stringify(e)).join("\n"),
    bytes = new TextEncoder().encode(text);
  async function* chunks() {
    for (const byte of bytes) yield Uint8Array.of(byte);
  }
  return Array.fromAsync(readDiffStream(chunks()));
}
test("decode byte-fragmented Unicode stream and preserve fold metadata", async () => {
  const file = createTestDiffFile();
  file.file.new_path = "变量.ts";
  file.diff.lhs_folds = [
    {
      tags: ["function"],
      range: {
        start: { line: 0, byte_column: 0 },
        end: { line: 2, byte_column: 8 },
      },
      placeholder: "…",
      summary: null,
      match_kind: "Novel",
    },
  ];
  const events = [{...start, files: [file.file]}, file, { type: "complete", succeeded: 1, failed: 0 }];
  expect((await decode(events)) as unknown).toEqual(events);
});
test("reject missing completion, counts, unknown version and invalid ordering", async () => {
  for (const events of [
    [start],
    [{ ...start, version: 2 }],
    [createTestDiffFile()],
    [start, { type: "complete", succeeded: 1, failed: 0 }],
    [start, start],
  ])
    await expect(decode(events)).rejects.toThrow();
});
test("file errors complete without discarding earlier successful results", async () => {
  const file = createTestDiffFile();
  const failedFile = {...file.file, old_path: "failed.ts", new_path: "failed.ts"};
  expect(
    (
      await decode([
        { ...start, total: 2, files: [file.file, failedFile] },
        file,
        { type: "file_error", file: failedFile, message: "unreadable" },
        { type: "complete", succeeded: 1, failed: 1 },
      ])
    ).length,
  ).toBe(4);
});

test("manifest validates count and identities while allowing results in arrival order", async () => {
  const a = createTestDiffFile(), b = createTestDiffFile();
  b.file = {...b.file, old_path: "b.ts", new_path: "b.ts"};
  for (const events of [
    [{...start, files: []}],
    [{...start, total: 2, files: [a.file, a.file]}],
    [{...start, total: 2, files: [a.file, b.file]}, b, b],
    [start, b],
  ]) await expect(decode(events)).rejects.toThrow();
  expect((await decode([{...start, total: 2, files: [a.file, b.file]}, b, a,
    {type: "complete", succeeded: 2, failed: 0}])).map(e => e.type))
    .toEqual(["start", "file", "file", "complete"]);
});
