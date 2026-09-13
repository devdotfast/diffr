import { expect, test } from "bun:test";
import { createTestDiffFile, fold, leaf, line } from "./fixture";
import { alignmentIds, defaultCollapsed, flatten, foldHeaders, foldIds, foldTint, gapIds, hiddenLines } from "./regions";
import { dark, rowsForFile } from "./rows";
import type { DiffFile, Region } from "./wire";
import { foldBackground } from "../ui/diff/CodeRowView";
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
  file.diff.stats = { textual: { added: 1, removed: 0 }, visible: { added: 1, removed: 0 } };
  return file;
}
test("folds keep headers visible, hide trailing lines only when nothing follows", () => {
  const file = createFoldedDiffFile();
  if (file.diff.type !== "text") throw new Error();
  const { folds, leaves } = flatten(file.diff);
  expect(folds[1].map((f) => [f.foldStateId, f.headerLine, f.lastHidden])).toEqual([[10, 0, 6], [11, 1, 3], [12, 5, 6]]);
  expect(folds[1][0].nested).toEqual([11, 12]);
  expect(leaves[1].map((l) => [l.foldStateId, l.startLine, l.endLine])).toEqual([[1, 0, 1], [2, 1, 3], [3, 3, 4], [4, 4, 5], [5, 5, 7], [6, 7, 8]]);
  expect([...hiddenLines(folds[1], new Set([11, 12]))].sort()).toEqual([2, 3, 6]);
  // A fold hidden inside a collapsed outer fold hides nothing of its own.
  expect([...hiddenLines(folds[1], new Set([10, 11]))].sort()).toEqual([1, 2, 3, 4, 5, 6]);
  expect(foldHeaders(folds[1], leaves[1], new Set([11]), alignmentIds(file.diff)[0]).get(1)).toEqual({ id: 11, label: "Body", collapsed: true, tint: "neutral" });
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
  expect(split[1].right!.fold).toEqual({ id: 11, label: "Body", collapsed: true, tint: "neutral" });
  expect(split[1].left!.fold).toEqual({ id: 11, label: "Body", collapsed: true, tint: "neutral" });
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
  expect(split[1].left!.fold).toEqual({ id: 1, label: "1 unchanged line", collapsed: true, tint: "neutral" });
  expect(split[1].left!.lineNumber).toBeUndefined();
  expect(split[1].right!.fold).toEqual({ id: 1, label: "1 unchanged line", collapsed: true, tint: "neutral" });
  expect(split.filter((r) => r.left?.fold?.id === 1)).toHaveLength(1);
  const unified = rowsForFile(file, 0, "unified", dark, new Set([1]));
  expect(unified[1].cell).toMatchObject({ fold: { id: 1, collapsed: true } });
  // Open, the leaf's first line carries the chevron so it can be collapsed again.
  expect(rowsForFile(file, 0, "split", dark, new Set())[1].left!.fold).toEqual({ id: 1, label: "1 unchanged line", collapsed: false, tint: "neutral" });
  // Unlabelled gaps get a computed label; ordinary leaves are not foldable.
  const tagged = createTestDiffFile();
  if (tagged.diff.type !== "text") throw new Error();
  tagged.diff.rhs!.regions[0].tags = ["unchanged"];
  const rows = rowsForFile(tagged, 0, "split", dark, new Set());
  expect(rows[1].right!.fold).toEqual({ id: 1, label: "1 unchanged lines", collapsed: false, tint: "neutral" });
  expect(rows[1].left!.fold).toBeUndefined();
  expect(rows[2].right!.fold).toBeUndefined();
});

test("a fold that runs to the end of the file may end one past its last line", () => {
  const file = createTestDiffFile();
  if (file.diff.type !== "text") throw new Error("fixture is not a text diff");
  const text = "fn a() {\n    b();\n}\n";
  // Three lines; the fold's hull ends at (3, 0), just past the last line.
  const regions: Region[] = [fold(10, [0, 0], [3, 0], [leaf(1, 0, 1), leaf(2, 1, 3)])];
  file.diff.lhs = { text, syntax: [], regions };
  file.diff.rhs = { text, syntax: [], regions };
  const { folds } = flatten(file.diff);
  expect(folds[0][0]?.lastHidden).toBe(2);
});

test("regions sharing a fold_state_id collapse and expand as one bundle", () => {
  const file = createTestDiffFile();
  if (file.diff.type !== "text") throw new Error("fixture is not a text diff");
  const text = [
    "/// Adds two numbers.", // 0
    "/// Wraps on overflow.", // 1
    "fn add(a: u8, b: u8) -> u8 {", // 2
    "    a.wrapping_add(b)", // 3
    "}", // 4
  ].join("\n") + "\n";
  // The docstring (alignment 20) and the function body (alignment 21) share fold state 21.
  const docstring: Region = { ...fold(20, [0, 0], [2, 0], [leaf(1, 0, 2)], "", ["comment"], true), fold_state_id: 21 };
  const body = fold(21, [2, 28], [4, 0], [leaf(2, 2, 4)], "// pseudocode\nreturn a + b", ["body", "function"], true);
  const regions: Region[] = [docstring, body, leaf(3, 4, 5)];
  file.diff.lhs = { text, syntax: [], regions };
  file.diff.rhs = { text, syntax: [], regions };
  const collapsed = defaultCollapsed(file.diff);
  expect([...collapsed]).toEqual([21]);
  const { folds } = flatten(file.diff);
  // One id hides both the docstring's continuation and the body.
  expect([...hiddenLines(folds[1], collapsed)].sort()).toEqual([1, 3]);
  expect([...hiddenLines(folds[1], new Set())]).toEqual([]);
  const rows = rowsForFile(file, 0, "split", dark, collapsed).filter((r) => r.right && !r.right.foldLabel);
  // The docstring is a one-row ⋯ fold (empty label), the signature stays visible, and both headers
  // carry the same id, so either chevron toggles the pair.
  const headers = rows.filter((r) => r.right!.fold).map((r) => [r.right!.lineNumber, r.right!.fold!.id, r.right!.fold!.label, r.right!.fold!.collapsed]);
  expect(headers).toEqual([[1, 21, "", true], [3, 21, "// pseudocode\nreturn a + b", true]]);
  expect(rows.map((r) => r.right!.lineNumber)).toEqual([1, 3, 5]);
});

/**
 * The shape diffr emits for a new, summarized function (src/category.rs `from_path`,
 * src/protocol/project.rs `syntax_spans`): a one-line docstring leaf tagged `docstring`, collapsed
 * with an empty label, sharing fold state 4 with the collapsed function fold after it, right side only.
 */
export function createBundledDiffFile(): DiffFile {
  const file = createTestDiffFile();
  if (file.diff.type !== "text") throw new Error("fixture is not a text diff");
  const text = [
    "];", // 0
    "/// The built-in rule for a path.", // 1
    "pub(crate) fn from_path(path: &str) -> Option<&str> {", // 2
    "    None", // 3
    "}", // 4
  ].join("\n") + "\n";
  const docstring: Region = {
    ...leaf(38, 1, 2), fold_state_id: 4, tags: ["docstring"], visibility: { collapsed: true, label: "" },
  };
  const body = fold(4, [2, 52], [4, 0], [leaf(5, 2, 4)], "// pseudocode\nreturn None", ["body", "function"], true);
  file.diff.rhs = { text, syntax: [], regions: [leaf(3, 0, 1), docstring, body, leaf(6, 4, 5)] };
  file.diff.lhs = undefined;
  return file;
}
test("a collapsed docstring leaf bundled with its function renders as a bare ⋯ row", () => {
  const file = createBundledDiffFile();
  if (file.diff.type !== "text") throw new Error("fixture is not a text diff");
  const collapsed = defaultCollapsed(file.diff);
  expect([...collapsed]).toEqual([4]);
  expect(foldIds(file.diff)).toContain(4);
  const rows = rowsForFile(file, 0, "split", dark, collapsed).filter((r) => r.right && !r.right.foldLabel);
  // Line 1 ("];"), then the docstring as one ⋯ fold row, then the signature carrying the body fold.
  expect(rows.map((r) => [r.right!.lineNumber, r.right!.fold?.id, r.right!.fold?.label, r.right!.fold?.collapsed]))
    .toEqual([[1, undefined, undefined, undefined], [undefined, 4, "", true], [3, 4, "// pseudocode\nreturn None", true], [5, undefined, undefined, undefined]]);
  // Right side only, so both the ⋯ row and the summary take the added tint.
  expect(rows.filter((r) => r.right!.fold).map((r) => r.right!.fold!.tint)).toEqual(["inserted", "inserted"]);
  // Expanding the shared id reveals the docstring line too.
  const open = rowsForFile(file, 0, "split", dark, new Set()).filter((r) => r.right && !r.right.foldLabel);
  expect(open.map((r) => r.right!.lineNumber)).toEqual([1, 2, 3, 4, 5]);
});

test("a collapsed fold takes its side's change tint when one-sided and stays neutral when paired", () => {
  const text = "fn a() {\n    x\n}\n";
  const body = (id: number, label: string) => fold(id, [0, 8], [2, 0], [leaf(id + 100, 0, 2)], label, ["body", "function"], true);
  const rowsFor = (lhs: Region[] | undefined, rhs: Region[] | undefined) => {
    const file = createTestDiffFile();
    if (file.diff.type !== "text") throw new Error();
    file.diff.lhs = lhs && { text, syntax: [], regions: [...lhs, leaf(9, 2, 3)] };
    file.diff.rhs = rhs && { text, syntax: [], regions: [...rhs, leaf(9, 2, 3)] };
    return rowsForFile(file, 0, "split", dark, defaultCollapsed(file.diff));
  };
  // Inserted: a summary on the right with no counterpart. The header and every label row are green.
  const inserted = rowsFor(undefined, [body(7, "// pseudocode\nreturn x")]);
  const header = inserted.find((r) => r.right?.fold)!.right!;
  expect(header.fold!.tint).toBe("inserted");
  expect(inserted.filter((r) => r.right?.foldLabel).map((r) => r.right!.foldTint)).toEqual(["inserted", "inserted"]);
  expect(foldBackground(dark, "inserted")).toBe(dark.addition);
  // Removed: the same body only on the left.
  const removed = rowsFor([body(7, "2 lines removed")], undefined);
  expect(removed.find((r) => r.left?.fold)!.left!.fold!.tint).toBe("removed");
  expect(foldBackground(dark, "removed")).toBe(dark.deletion);
  // Paired: the same alignment id on both sides keeps the neutral fold background.
  const paired = rowsFor([body(7, "Body")], [body(7, "Body")]);
  const both = paired.find((r) => r.left?.fold && r.right?.fold)!;
  expect([both.left!.fold!.tint, both.right!.fold!.tint]).toEqual(["neutral", "neutral"]);
  expect(foldBackground(dark, "neutral")).toBe(dark.foldBackground);
  expect(foldTint(7, 1, new Set([7]))).toBe("neutral");
  expect(foldTint(7, 1, new Set())).toBe("inserted");
  expect(foldTint(7, 0, new Set())).toBe("removed");
});
