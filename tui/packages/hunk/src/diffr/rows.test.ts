import { expect, test } from "bun:test";
import { createTestDiffFile, leaf, line, withIdenticalLines } from "./fixture";
import { captureColor, dark, light, lineSpans, rowsForFile } from "./rows";
import { measureRows, visibleRows } from "./geometry";
import { copySelection } from "./selection";
test("split zips leaves on their ids, unified groups old before new", () => {
  const file = createTestDiffFile();
  const split = rowsForFile(file, 0, "split", dark).filter((r) => r.left);
  expect(split.map((r) => [r.left!.lineNumber, r.right!.lineNumber])).toEqual([
    [1, 1],
    [2, 2],
    [undefined, 3],
    [3, 4],
  ]);
  expect(split.map((r) => r.right!.kind)).toEqual(["context", "addition", "addition", "context"]);
  const unified = rowsForFile(file, 0, "unified", dark).filter((r) => r.cell);
  expect(
    unified.map(
      (r) => r.cell!.sign + r.cell!.spans.map((s) => s.text).join(""),
    ),
  ).toEqual([
    " start();",
    '-send("old");',
    '+send("new");',
    "+extra();",
    " finish();",
  ]);
  expect(rowsForFile(file, 0, "split", dark).filter((r) => r.hunkStart)).toHaveLength(1);
});
test("moved code renders as moved on both copies, labelled with where the other copy starts", () => {
  const file = createTestDiffFile();
  if (file.diff.type !== "text") throw new Error();
  // `b` moved above `a`, and its line was also edited.
  file.diff.lhs = { text: "a\nb\n", syntax: [], regions: [leaf(1, 0, 1), leaf(2, 1, 2, [line(1, 0, 1)])] };
  file.diff.rhs = { text: "b2\na\n", syntax: [], regions: [leaf(2, 0, 1, [line(0, 0, 2)]), leaf(1, 1, 2)] };
  const rows = rowsForFile(file, 0, "split", dark).filter((r) => r.left);
  const text = (c: { spans: { text: string }[] }) => c.spans.map((s) => s.text).join("");
  expect(rows.map((r) => [r.left!.moveLabel ? text(r.left!) : r.left!.lineNumber,
    r.right!.moveLabel ? text(r.right!) : r.right!.lineNumber])).toEqual([
    [undefined, "moved from line 2"], [undefined, 1], [1, 2], ["moved to line 1", undefined], [2, undefined],
  ]);
  const movedRight = rows[1].right!, movedLeft = rows[4].left!;
  // No +/−, the moved tint instead of added or removed, and each copy points at the other.
  expect([movedRight.kind, movedRight.sign, movedRight.moveKind, movedRight.jump]).toEqual(
    ["context", " ", "moved", { side: "left", line: 2 }]);
  expect([movedLeft.kind, movedLeft.sign, movedLeft.moveKind, movedLeft.jump]).toEqual(
    ["context", " ", "moved", { side: "right", line: 1 }]);
  expect(rows[0].right!.jump).toEqual({ side: "left", line: 2 });
  // The edit inside the moved copy still gets the strong word tint.
  expect(movedRight.spans.some((s) => s.bg === dark.addWord)).toBe(true);
  expect(dark.moved).not.toBe(dark.addition);
  expect(dark.moved).not.toBe(dark.deletion);
  // The unchanged line between the copies is plain context.
  expect([rows[2].left!.moveKind, rows[2].left!.kind]).toEqual([undefined, "context"]);
});
test("changed spans paint the darker word tint, distinct from the line tint", () => {
  for (const theme of [dark, light]) {
    const spans = lineSpans("let x = old + y;", [], [line(0, 8, 11)], "right", theme);
    expect(spans.map((s) => [s.text, s.bg])).toEqual([
      ["let x = ", undefined], ["old", theme.addWord], [" + y;", undefined],
    ]);
    expect(theme.addWord).not.toBe(theme.addition);
    expect(lineSpans("old", [], [line(0, 0, 3)], "left", theme)[0].bg).toBe(theme.deleteWord);
    expect(theme.deleteWord).not.toBe(theme.deletion);
  }
});
test("every line of a novel leaf is tinted, even without a span", () => {
  const file = createTestDiffFile();
  if (file.diff.type !== "text") throw new Error();
  file.diff.lhs = { text: "a\n", syntax: [], regions: [leaf(1, 0, 1)] };
  // A new block: one changed word, a blank line, and an unpaired leaf with no spans at all.
  file.diff.rhs = { text: "a\nb = 1\n\nc\n", syntax: [], regions: [leaf(1, 0, 1), leaf(2, 1, 3, [line(1, 4, 5)]), leaf(3, 3, 4)] };
  const rows = rowsForFile(file, 0, "split", dark).filter((r) => r.right);
  expect(rows.map((r) => [r.right!.kind, r.right!.spans.some((s) => s.bg)])).toEqual([
    ["context", false], ["addition", true], ["addition", false], ["addition", false],
  ]);
});
test("byte spans survive multibyte characters and tabs; captures pick theme colours", () => {
  const spans = lineSpans(
    "é\t变量",
    [{ line: 0, start_column: 3, end_column: 9, capture: "type.builtin" }],
    [line(0, 3, 9)],
    "right",
    dark,
  );
  expect(spans.map((s) => s.text).join("")).toBe("é   变量");
  expect(spans.at(-1)!.bg).toBe(dark.addWord);
  expect(spans.at(-1)!.fg).toBe(dark.syntax("type")!);
  expect(() => lineSpans("é", [], [line(0, 1, 9)], "left", dark)).toThrow();
  expect(captureColor("function.method", dark)).toBe(dark.syntax("function")!);
  expect(captureColor("unknown.thing", dark)).toBe(dark.fg);
});
test("the innermost syntax capture colours a nested span", () => {
  const spans = lineSpans(
    'f("x")',
    [
      { line: 0, start_column: 0, end_column: 6, capture: "function.call" },
      { line: 0, start_column: 2, end_column: 5, capture: "string" },
    ],
    [],
    "left",
    dark,
  );
  expect(spans.map((s) => [s.text, s.fg])).toEqual([
    ['f(', dark.syntax("function")], ['"x"', dark.syntax("string")], [")", dark.syntax("function")],
  ]);
});
test("wrapping adds equal split heights and windowing mounts only intersecting rows", () => {
  const rows = rowsForFile(createTestDiffFile(), 0, "split", dark);
  const geometry = measureRows(rows, 20, true, 0);
  expect(geometry.rows[2].height).toBeGreaterThan(1);
  expect(geometry.rows[2].height).toBe(
    Math.max(geometry.rows[2].left.length, geometry.rows[2].right.length),
  );
  expect(visibleRows(geometry, 0, 2).length).toBe(2);
});
test("selection copies one source side and excludes padding and added lines", () => {
  const file = createTestDiffFile(),
    rows = rowsForFile(file, 0, "split", dark);
  expect(
    copySelection([file], rows, {
      anchor: rows[1].key,
      end: rows.at(-1)!.key,
      side: "left",
    }),
  ).toBe('start();\nsend("old");\nfinish();');
});
test("large file mounts a bounded viewport", () => {
  const file = withIdenticalLines(createTestDiffFile(), 20000);
  const geometry = measureRows(rowsForFile(file, 0, "split", dark), 120, false, 0);
  expect(visibleRows(geometry, 10000, 40)).toHaveLength(40);
});
test("context gaps come from collapsed unchanged leaves, one row per gap", () => {
  const file = createTestDiffFile();
  if (file.diff.type !== "text") throw new Error();
  const lines = Array.from({ length: 12 }, (_, i) => `line ${i}`);
  const gap = (id: number, start: number, end: number) => {
    const region = leaf(id, start, end);
    region.tags = ["unchanged"];
    region.visibility = { collapsed: true, label: `${end - start} unchanged lines` };
    return region;
  };
  const regions = () => [gap(1, 0, 1), leaf(2, 1, 3, [line(1, 0, 6)]), gap(3, 3, 8), leaf(4, 8, 10), gap(5, 10, 12)];
  file.diff.lhs = { text: lines.join("\n"), syntax: [], regions: regions() };
  file.diff.rhs = { text: lines.join("\n"), syntax: [], regions: regions() };
  const split = rowsForFile(file, 0, "split", dark, new Set([1, 3, 5]));
  const shown = (fold: { label: string; collapsed: boolean } | undefined, line: number | undefined) =>
    fold?.collapsed ? fold.label : line;
  expect(split.slice(1).map(r => shown(r.left?.fold, r.left?.lineNumber)))
    .toEqual(["1 unchanged lines", 2, 3, "5 unchanged lines", 9, 10, "2 unchanged lines"]);
  // Unified shows the novel leaf's lines once per side, removals first.
  const unified = rowsForFile(file, 0, "unified", dark, new Set([1, 3, 5]));
  expect(unified.slice(1).map(r => shown(r.cell?.fold, r.cell?.newLineNumber ?? r.cell?.oldLineNumber)))
    .toEqual(["1 unchanged lines", 2, 3, 2, 3, "5 unchanged lines", 9, 10, "2 unchanged lines"]);
  for (const layout of ["split", "unified"] as const) {
    expect(rowsForFile(file, 0, layout, dark, new Set([1, 3, 5])).filter(r => r.hunkStart)).toHaveLength(1);
    expect(rowsForFile(file, 0, layout, dark, new Set()).length).toBeGreaterThanOrEqual(13);
  }
});
test("unified trusts diffr's changed spans despite different source indentation", () => {
  const file = createTestDiffFile();
  if (file.diff.type !== "text") throw new Error();
  file.diff.lhs = { text: "  call();\n", syntax: [], regions: [leaf(1, 0, 1)] };
  file.diff.rhs = { text: "    call();\n", syntax: [], regions: [leaf(1, 0, 1)] };
  const rows = rowsForFile(file, 0, "unified", dark);
  expect(rows).toHaveLength(2);
  expect(rows[1].cell).toMatchObject({kind: "context", sign: " ", oldLineNumber: 1, newLineNumber: 1});
  expect(rows[1].cell!.spans.map(s => s.text).join("")).toBe("    call();");
  for (const side of ["left", "right"] as const) {
    expect(copySelection([file], rows, {anchor: rows[1].key, end: rows[1].key, side}))
      .toBe(side === "left" ? "  call();" : "    call();");
  }
  // Only the side with a changed span may receive change styling.
  file.diff.rhs.regions = [leaf(1, 0, 1, [line(0, 0, 11)])];
  const changed = rowsForFile(file, 0, "unified", dark).slice(1);
  expect(changed.map(r => r.cell!.kind)).toEqual(["context", "addition"]);
});
test("binary and one-sided files render without a second side", () => {
  const file = createTestDiffFile();
  file.diff = { type: "binary", lhs: { size: 10 }, rhs: { size: 12 } };
  expect(rowsForFile(file, 0, "split", dark).map((r) => r.label)).toEqual(["demo.ts", "Binary file"]);
  const added = createTestDiffFile();
  added.file = { rhs: added.file.rhs };
  if (added.diff.type !== "text") throw new Error();
  added.diff = { type: "text", rhs: { text: "new\n", syntax: [], regions: [leaf(1, 0, 1, [line(0, 0, 3)])] },
    stats: { textual: { added: 1, removed: 0 }, visible: { added: 1, removed: 0 } } };
  const rows = rowsForFile(added, 0, "split", dark).filter((r) => r.left);
  expect(rows.map((r) => [r.left!.kind, r.right!.lineNumber])).toEqual([["empty", 1]]);
});
