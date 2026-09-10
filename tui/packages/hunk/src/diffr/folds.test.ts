import { expect, test } from "bun:test";
import { createTestDiffFile } from "./fixture";
import { foldRegions, hiddenLines, nestedRegions } from "./folds";
import { dark, rowsForFile } from "./rows";
import type { DiffFile } from "./wire";
/** Rust-style body folds: header and closing brace stay visible, like VS Code. */
export function createFoldedDiffFile(): DiffFile {
  const file = createTestDiffFile();
  const lines = [
    "fn outer() {",      // 0
    "    inner(|| {",    // 1
    "        a();",      // 2
    "        b();",      // 3
    "    });",           // 4
    "    // trailing",   // 5
    "    // comment",    // 6
    "}",                 // 7
  ];
  const text = lines.join("\n") + "\n";
  file.diff.lhs_src = file.diff.rhs_src = { Text: text };
  file.diff.lhs_positions = file.diff.rhs_positions = [];
  file.diff.aligned_rows = lines.map((_, i) => [i, i]);
  file.diff.hunks = [{ novel_lhs: [], novel_rhs: [3], lines: file.diff.aligned_rows }];
  const range = (a: [number, number], b: [number, number]) => ({
    start: { line: a[0], byte_column: a[1] },
    end: { line: b[0], byte_column: b[1] },
  });
  const paired = (r: ReturnType<typeof range>, placeholder: string, tags: string[]) => ({
    tags, range: r, match_kind: { Unchanged: { opposite: r } }, placeholder,
  });
  file.diff.rhs_folds = [
    paired(range([0, 12], [7, 0]), "Body", ["body"]),
    paired(range([1, 14], [4, 4]), "Body", ["body"]),
    // A whole-node fold ending at end of line hides its last line too.
    paired(range([5, 4], [6, 14]), "Comment", ["comment"]),
    // Single-line folds are never foldable.
    paired(range([2, 8], [2, 11]), "Call", ["call"]),
  ];
  file.diff.lhs_folds = file.diff.rhs_folds;
  return file;
}
test("regions keep headers visible, hide trailing lines only when nothing follows", () => {
  const regions = foldRegions(createFoldedDiffFile().diff);
  const right = regions.filter((r) => r.side === 1);
  expect(right.map((r) => [r.id, r.startLine, r.endLine, r.hideEnd])).toEqual([
    ["R0:7", 0, 7, false],
    ["R1:4", 1, 4, false],
    ["R5:6", 5, 6, true],
  ]);
  // Unchanged folds share ids across sides so both collapse together.
  expect(regions.filter((r) => r.side === 0).map((r) => r.id)).toEqual(["R0:7", "R1:4", "R5:6"]);
  expect(nestedRegions(right, right[0]).map((r) => r.id)).toEqual(["R1:4", "R5:6"]);
});
test("collapsed regions hide each side's lines between header and close", () => {
  const diff = createFoldedDiffFile().diff;
  const regions = foldRegions(diff);
  const hidden = hiddenLines(diff.aligned_rows, regions, new Set(["R1:4", "R5:6"]), () => true);
  expect(hidden.map((side) => [...side].sort())).toEqual([[2, 3, 6], [2, 3, 6]]);
  // A fold whose header the viewer does not show cannot hide anything.
  expect(hiddenLines(diff.aligned_rows, regions, new Set(["R1:4"]), () => false)[1].size).toBe(0);
});
test("a fold on one side leaves the other side's lines beside blank cells", () => {
  const file = createFoldedDiffFile();
  // The right-hand closure body is novel; the left keeps two lines aligned inside it.
  file.diff.rhs_folds = [{
    tags: ["body"], placeholder: "Body", match_kind: "Novel",
    range: { start: { line: 1, byte_column: 14 }, end: { line: 4, byte_column: 4 } },
  }];
  file.diff.lhs_folds = [];
  const rows = rowsForFile(file, 0, "split", dark, true, new Set(["R1:4"])).filter((r) => r.left);
  expect(rows.map((r) => [r.left!.lineNumber, r.right!.lineNumber])).toEqual([
    [1, 1], [2, 2], [3, undefined], [4, undefined], [5, 5], [6, 6], [7, 7], [8, 8],
  ]);
  expect(rows[2].right!.kind).toBe("empty");
  expect(rows[1].right!.fold?.collapsed).toBe(true);
  expect(rows[1].left!.fold).toBeUndefined();
});
test("rows carry fold headers on both layouts and drop hidden lines", () => {
  const file = createFoldedDiffFile();
  const split = rowsForFile(file, 0, "split", dark, true, new Set(["R1:4"])).filter((r) => r.right);
  expect(split.map((r) => r.right!.lineNumber)).toEqual([1, 2, 5, 6, 7, 8]);
  expect(split[1].right!.fold).toEqual({ id: "R1:4", placeholder: "Body", collapsed: true });
  expect(split[1].left!.fold).toEqual({ id: "R1:4", placeholder: "Body", collapsed: true });
  expect(split[0].right!.fold?.collapsed).toBe(false);
  const unified = rowsForFile(file, 0, "unified", dark, true, new Set(["R0:7"])).filter((r) => r.cell);
  expect(unified.map((r) => r.cell!.newLineNumber)).toEqual([1, 8]);
  expect(unified[0].cell!.fold?.id).toBe("R0:7");
});
