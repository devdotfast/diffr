import { expect, test } from "bun:test";
import { readDiffStream } from "./stream";
import { createTestDiffFile } from "./fixture";
const start = {
  type: "start",
  version: 1,
  before: { kind: "index" },
  after: { kind: "working_tree" },
  total: 1,
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
      match_kind: "Novel",
    },
  ];
  const events = [start, file, { type: "complete", succeeded: 1, failed: 0 }];
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
  expect(
    (
      await decode([
        { ...start, total: 2 },
        file,
        { type: "file_error", file: file.file, message: "unreadable" },
        { type: "complete", succeeded: 1, failed: 1 },
      ])
    ).length,
  ).toBe(4);
});
