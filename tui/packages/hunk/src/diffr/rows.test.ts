import { expect, test } from "bun:test";
import { createTestDiffFile, fold, leaf, line, withIdenticalLines } from "./fixture";
import type { Region, Span } from "./wire";
import { captureColor, dark, light, lineSpans, rowsForFile } from "./rows";
import { pairedIds } from "./regions";
import { measureRows, visibleRows } from "./geometry";
import { copySelection } from "./selection";
test("split zips leaves on their alignment ids, unified groups old before new", () => {
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
test("matched folds zip nothing themselves: rows come from leaves, and the pair toggles by fold state", () => {
  const file = createTestDiffFile();
  if (file.diff.type !== "text") throw new Error();
  // `b` moved above `a`: the two `b` folds share fold state 3, each with its own id, and
  // none of their lines line up. `a` stayed, its leaf paired row for row.
  const matched = (id: number, start: number, leafId: number, novel: Span[]) =>
    ({ ...fold(id, [start, 0], [start + 2, 0], [leaf(leafId, start, start + 2, novel)]), fold_state_id: 3 });
  file.diff.lhs = { text: "a {\n}\nb {\n}\n", syntax: [],
    regions: [fold(1, [0, 0], [2, 0], [leaf(2, 0, 2)]), matched(3, 2, 4, [line(2, 0, 3), line(3, 0, 1)])] };
  file.diff.rhs = { text: "b {\n}\na {\n}\n", syntax: [],
    regions: [matched(5, 0, 6, [line(0, 0, 3), line(1, 0, 1)]), { ...fold(7, [2, 0], [4, 0], [leaf(2, 2, 4)]), fold_state_id: 1 }] };
  const lines = (collapsed: Set<number>) => rowsForFile(file, 0, "split", dark, collapsed).filter((r) => r.left)
    .map((r) => [r.left!.lineNumber, r.right!.lineNumber]);
  expect(lines(new Set())).toEqual([[undefined, 1], [undefined, 2], [1, 3], [2, 4], [3, undefined], [4, undefined]]);
  const rows = rowsForFile(file, 0, "split", dark).filter((r) => r.left);
  // diffr sent the moved lines as new and removed; the frontend does not repaint them.
  expect([rows[0].right!.kind, rows[4].left!.kind]).toEqual(["addition", "deletion"]);
  expect([rows[2].left!.kind, rows[2].right!.kind]).toEqual(["context", "context"]);
  // Collapsing fold state 3 folds both copies; each is one row of its own, hiding every
  // line it covers, and `a` still zips row for row between them.
  expect(lines(new Set([3]))).toEqual([[undefined, undefined], [1, 3], [2, 4], [undefined, undefined]]);
});
test("rows zip leaves by alignment_id, and a paired leaf's tint follows its own id", () => {
  const file = createTestDiffFile();
  if (file.diff.type !== "text") throw new Error();
  // As diffr numbers them: ids are unique across sides, alignments and fold states are not ids.
  const leafOf = (id: number, alignment: number, state: number, start: number, end: number, changed: Span[] = []): Region =>
    ({ ...leaf(id, start, end, changed), alignment_id: alignment, fold_state_id: state });
  file.diff.lhs = { text: "a\nb\n", syntax: [], regions: [leafOf(0, 0, 0, 0, 1), leafOf(1, 1, 1, 1, 2)] };
  file.diff.rhs = { text: "a\nnew\nb\n", syntax: [],
    regions: [leafOf(2, 0, 0, 0, 1), leafOf(3, 2, 3, 1, 2, [line(1, 0, 3)]), leafOf(4, 1, 1, 2, 3)] };
  const rows = rowsForFile(file, 0, "split", dark).filter((r) => r.left);
  expect(rows.map((r) => [r.left!.lineNumber, r.right!.lineNumber])).toEqual([[1, 1], [undefined, 2], [2, 3]]);
  expect(pairedIds(file.diff).map((ids) => [...ids].sort())).toEqual([[0, 1], [2, 4]]);
  // A collapsed paired leaf stays neutral on both sides; a collapsed one-sided leaf takes the added tint.
  for (const region of [file.diff.lhs.regions[1]!, file.diff.rhs.regions[2]!, file.diff.rhs.regions[1]!])
    region.visibility = { collapsed: true, label: "hidden" };
  const collapsed = rowsForFile(file, 0, "split", dark, new Set([1, 3]));
  const tints = collapsed.flatMap((r) => [r.left?.fold?.tint, r.right?.fold?.tint]).filter((tint) => tint);
  expect(tints).toEqual(["inserted", "neutral", "neutral"]);
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
