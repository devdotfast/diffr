import { expect, test } from "bun:test";
import { createTestDiffFile, fold, leaf, line } from "./fixture";
import { defaultCollapsed, flatten, foldHeaders, gapIds, hiddenLines } from "./regions";
import { dark, rowsForFile } from "./rows";
import type { DiffFile, Region } from "./wire";
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
  // ids: 10 outer body, 11 closure body, 12 comment; leaves 1..5 tile the file.
  const regions = (changed: boolean): Region[] => [
    fold(10, [0, 12], [7, 0], [
      leaf(1, 0, 1),
      fold(11, [1, 14], [4, 4], [leaf(2, 1, 3), leaf(3, 3, 4, changed ? [line(3, 8, 12)] : []), leaf(4, 4, 5)]),
      // A whole-node fold ending at end of line hides its last line too.
      fold(12, [5, 4], [6, 14], [leaf(5, 5, 7)], "Comment", ["comment"]),
      leaf(6, 7, 8),
    ]),
  ];
  if (file.diff.type !== "text") throw new Error("fixture is not a text diff");
  file.diff.lhs = { text, syntax: [], regions: regions(false) };
  file.diff.rhs = { text, syntax: [], regions: regions(true) };
  return file;
}
test("folds keep headers visible, hide trailing lines only when nothing follows", () => {
  const file = createFoldedDiffFile();
  if (file.diff.type !== "text") throw new Error();
  const { folds, leaves } = flatten(file.diff);
  expect(folds[1].map((f) => [f.id, f.headerLine, f.lastHidden])).toEqual([[10, 0, 6], [11, 1, 3], [12, 5, 6]]);
  expect(folds[1][0].nested).toEqual([11, 12]);
  expect(leaves[1].map((l) => [l.id, l.startLine, l.endLine])).toEqual([[1, 0, 1], [2, 1, 3], [3, 3, 4], [4, 4, 5], [5, 5, 7], [6, 7, 8]]);
  expect([...hiddenLines(folds[1], new Set([11, 12]))].sort()).toEqual([2, 3, 6]);
  // A fold hidden inside a collapsed outer fold hides nothing of its own.
  expect([...hiddenLines(folds[1], new Set([10, 11]))].sort()).toEqual([1, 2, 3, 4, 5, 6]);
  expect(foldHeaders(folds[1], leaves[1], new Set([11])).get(1)).toEqual({ id: 11, label: "Body", collapsed: true });
});
test("visibility seeds collapsed ids and tags name the context gaps", () => {
  const file = createFoldedDiffFile();
  if (file.diff.type !== "text") throw new Error();
  file.diff.rhs!.regions[0].visibility = { collapsed: true, label: "Body" };
  const gap = leaf(6, 7, 8);
  gap.tags = ["unchanged"];
  gap.visibility = { collapsed: true, label: "1 unchanged line" };
  file.diff.rhs!.regions[0].children[3] = gap;
  expect([...defaultCollapsed(file.diff)]).toEqual([6, 10]);
  expect(gapIds(file.diff)).toEqual([6]);
});
test("a fold on one side leaves the other side's lines beside blank cells", () => {
  const file = createFoldedDiffFile();
  if (file.diff.type !== "text") throw new Error();
  // The right-hand closure body is a novel fold with its own leaves; the left keeps flat leaves.
  file.diff.lhs!.regions = [leaf(1, 0, 1), leaf(2, 1, 3), leaf(3, 3, 4), leaf(4, 4, 5), leaf(5, 5, 7), leaf(6, 7, 8)];
  const rows = rowsForFile(file, 0, "split", dark, new Set([11])).filter((r) => r.left);
  expect(rows.map((r) => [r.left!.lineNumber, r.right!.lineNumber])).toEqual([
    [1, 1], [2, 2], [3, undefined], [4, undefined], [5, 5], [6, 6], [7, 7], [8, 8],
  ]);
  expect(rows[2].right!.kind).toBe("empty");
  expect(rows[1].right!.fold?.collapsed).toBe(true);
  expect(rows[1].left!.fold).toBeUndefined();
});
test("rows carry fold headers on both layouts and drop hidden lines", () => {
  const file = createFoldedDiffFile();
  const split = rowsForFile(file, 0, "split", dark, new Set([11])).filter((r) => r.right);
  expect(split.map((r) => r.right!.lineNumber)).toEqual([1, 2, 5, 6, 7, 8]);
  expect(split[1].right!.fold).toEqual({ id: 11, label: "Body", collapsed: true });
  expect(split[1].left!.fold).toEqual({ id: 11, label: "Body", collapsed: true });
  expect(split[0].right!.fold?.collapsed).toBe(false);
  const unified = rowsForFile(file, 0, "unified", dark, new Set([10])).filter((r) => r.cell);
  expect(unified.map((r) => r.cell!.newLineNumber)).toEqual([1, 8]);
  expect(unified[0].cell!.fold?.id).toBe(10);
});
test("a multi-line label hangs under the collapsed header inside the fold tint", () => {
  const file = createFoldedDiffFile();
  if (file.diff.type !== "text") throw new Error();
  const pseudocode = "# pseudocode\ncall a\ncall b";
  file.diff.rhs!.regions[0].children[1].visibility = { collapsed: false, label: pseudocode };
  file.diff.lhs!.regions[0].children[1].visibility = { collapsed: false, label: pseudocode };
  const rows = rowsForFile(file, 0, "split", dark, new Set([11]));
  const labels = rows.filter((r) => r.right?.foldLabel);
  expect(labels.map((r) => r.right!.spans[0].text)).toEqual(["        # pseudocode", "        call a", "        call b"]);
  expect(labels.map((r) => r.left!.spans[0].text)).toEqual(["        # pseudocode", "        call a", "        call b"]);
  expect(rows.indexOf(labels[0])).toBe(rows.findIndex((r) => r.right?.fold?.id === 11) + 1);
  const unified = rowsForFile(file, 0, "unified", dark, new Set([11]));
  expect(unified.filter((r) => r.cell?.foldLabel)).toHaveLength(3);
});
test("a collapsed leaf is one fold row with the chevron, its label, and no line number", () => {
  const file = createTestDiffFile();
  if (file.diff.type !== "text") throw new Error();
  file.diff.lhs!.regions[0].visibility = file.diff.rhs!.regions[0].visibility = { collapsed: true, label: "1 unchanged line" };
  const split = rowsForFile(file, 0, "split", dark, new Set([1]));
  expect(split[1].left!.fold).toEqual({ id: 1, label: "1 unchanged line", collapsed: true });
  expect(split[1].left!.lineNumber).toBeUndefined();
  expect(split[1].right!.fold).toEqual({ id: 1, label: "1 unchanged line", collapsed: true });
  expect(split.filter((r) => r.left?.fold?.id === 1)).toHaveLength(1);
  const unified = rowsForFile(file, 0, "unified", dark, new Set([1]));
  expect(unified[1].cell).toMatchObject({ fold: { id: 1, collapsed: true } });
  // Open, the leaf's first line carries the chevron so it can be collapsed again.
  expect(rowsForFile(file, 0, "split", dark, new Set())[1].left!.fold).toEqual({ id: 1, label: "1 unchanged line", collapsed: false });
  // Unlabelled gaps get a computed label; ordinary leaves are not foldable.
  const tagged = createTestDiffFile();
  if (tagged.diff.type !== "text") throw new Error();
  tagged.diff.rhs!.regions[0].tags = ["unchanged"];
  const rows = rowsForFile(tagged, 0, "split", dark, new Set());
  expect(rows[1].right!.fold).toEqual({ id: 1, label: "1 unchanged lines", collapsed: false });
  expect(rows[1].left!.fold).toBeUndefined();
  expect(rows[2].right!.fold).toBeUndefined();
});
