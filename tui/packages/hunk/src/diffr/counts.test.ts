import { expect, test } from "bun:test";
import { blockBar, comparisonLabel, visibleCounts } from "./counts";
import { createTestDiffFile } from "./fixture";
import { createFoldedDiffFile } from "./regions.test";
test("visible counts start from the wire and move with folds, gaps, and closed files", () => {
  const file = createTestDiffFile();
  expect(visibleCounts(file, new Set())).toEqual({ added: 2, removed: 1 });
  // diffr's number is the anchor: with nothing toggled it is shown verbatim.
  if (file.diff.type !== "text") throw new Error();
  file.diff.stats.visible = { added: 5, removed: 4 };
  expect(visibleCounts(file, new Set())).toEqual({ added: 5, removed: 4 });
  expect(visibleCounts(file, new Set([2]))).toEqual({ added: 4, removed: 3 });
  file.diff.stats.visible = { added: 2, removed: 1 };
  expect(visibleCounts(file, new Set(), true)).toEqual({ added: 0, removed: 0 });
  // Collapsing the changed leaf itself hides its lines.
  expect(visibleCounts(file, new Set([2]))).toEqual({ added: 1, removed: 0 });
  const folded = createFoldedDiffFile();
  // rhs line 3 changed inside the closure body (fold 11).
  expect(visibleCounts(folded, new Set())).toEqual({ added: 1, removed: 0 });
  expect(visibleCounts(folded, new Set([11]))).toEqual({ added: 0, removed: 0 });
  expect(visibleCounts(folded, new Set([10]))).toEqual({ added: 0, removed: 0 });
  expect(visibleCounts(folded, new Set([12]))).toEqual({ added: 1, removed: 0 });
});
test("the block bar splits five blocks by share and keeps a block for any non-zero side", () => {
  expect(blockBar({ added: 0, removed: 0 })).toEqual(["neutral", "neutral", "neutral", "neutral", "neutral"]);
  expect(blockBar({ added: 10, removed: 0 })).toEqual(["added", "added", "added", "added", "added"]);
  expect(blockBar({ added: 3, removed: 1 })).toEqual(["added", "added", "added", "removed", "neutral"]);
  expect(blockBar({ added: 1, removed: 1 })).toEqual(["added", "added", "removed", "removed", "neutral"]);
  expect(blockBar({ added: 99, removed: 1 })).toEqual(["added", "added", "added", "added", "removed"]);
  expect(blockBar({ added: 13, removed: 6 })).toEqual(["added", "added", "added", "removed", "neutral"]);
  expect(blockBar({ added: 1, removed: 99 })).toEqual(["added", "removed", "removed", "removed", "removed"]);
});
test("comparison labels shorten shas and name the special snapshots", () => {
  expect(comparisonLabel({ type: "revision", rev: "main" }, { type: "revision", rev: "0123456789abcdef0123456789abcdef01234567" }))
    .toBe("main…0123456");
  expect(comparisonLabel({ type: "index" }, { type: "working_tree" })).toBe("index…working tree");
  expect(comparisonLabel({ type: "empty_tree" }, { type: "path", path: "after.ts" })).toBe("empty tree…after.ts");
});
