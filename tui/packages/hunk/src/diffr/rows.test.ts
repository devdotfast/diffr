import { expect, test } from "bun:test";
import { createTestDiffFile } from "./fixture";
import { dark, lineSpans, rowsForFile } from "./rows";
import { measureRows, visibleRows } from "./geometry";
import { copySelection } from "./selection";
test("split uses supplied pairs and padding, unified groups old before new", () => {
  const file = createTestDiffFile();
  const split = rowsForFile(file, 0, "split", dark).filter((r) => r.left);
  expect(split.map((r) => [r.left!.lineNumber, r.right!.lineNumber])).toEqual([
    [1, 1],
    [2, 2],
    [undefined, 3],
    [3, 4],
  ]);
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
});
test("byte spans survive multibyte characters and tabs", () => {
  const spans = lineSpans(
    "é\t变量",
    [
      {
        pos: { line: 0, start_col: 3, end_col: 9 },
        kind: { Novel: { highlight: { Atom: "Type" } } },
      },
    ],
    "right",
    dark,
  );
  expect(spans.map((s) => s.text).join("")).toBe("é   变量");
  expect(spans.at(-1)!.bg).toBe(dark.addWord);
  expect(() =>
    lineSpans(
      "é",
      [
        {
          pos: { line: 0, start_col: 1, end_col: 2 },
          kind: { Novel: { highlight: "Delimiter" } },
        },
      ],
      "left",
      dark,
    ),
  ).toThrow();
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
  const file = createTestDiffFile();
  const lines = Array.from({ length: 20000 }, (_, i) => `line ${i}`);
  file.diff.lhs_src = file.diff.rhs_src = { Text: lines.join("\n") };
  file.diff.lhs_positions = file.diff.rhs_positions = [];
  file.diff.hunks[0] = {
    novel_lhs: [],
    novel_rhs: [],
    lines: lines.map((_, i) => [i, i]),
  };
  file.diff.aligned_rows = lines.map((_, i) => [i, i]);
  const geometry = measureRows(
    rowsForFile(file, 0, "split", dark),
    120,
    false,
    0,
  );
  expect(visibleRows(geometry, 10000, 40)).toHaveLength(40);
});

test("full-file rows include source outside hunks and retain navigation without headers", () => {
  const file = createTestDiffFile();
  file.diff.hunks[0].lines = [[1, 1], [null, 2]];
  for (const layout of ["split", "unified"] as const) {
    const rows = rowsForFile(file, 0, layout, dark, true);
    expect(rows.filter(r => r.label).map(r => r.label)).toEqual(["demo.ts"]);
    expect(rows.filter(r => r.hunkStart)).toHaveLength(1);
    expect(copySelection([file], rows, {anchor: rows[1].key, end: rows.at(-1)!.key, side: "left"}))
      .toBe(file.diff.lhs_src === "Binary" ? "" : file.diff.lhs_src.Text.trimEnd());
  }
  file.diff.hunks = [];
  expect(rowsForFile(file, 0, "split", dark, true)).toHaveLength(5);
});

test("unchanged string literals use the normal text color", () => {
  const span = { line: 0, start_col: 0, end_col: 3 };
  const result = lineSpans('"x"', [{
    pos: span,
    kind: { UnchangedToken: {
      highlight: { Atom: { String: "StringLiteral" } },
      self_pos: [span], opposite_pos: [span],
    } },
  }], "right", dark);
  expect(result).toEqual([{ text: '"x"', fg: dark.fg, bg: undefined }]);
});

test("matched tokens keep syntax styling even when their counterpart is on another row", () => {
  const pos = { line: 2, start_col: 0, end_col: 3 };
  const positions = [{
    pos, kind: { UnchangedToken: {
      highlight: "Delimiter" as const, self_pos: [pos],
      opposite_pos: [{ ...pos, line: 8 }],
    } },
  }];
  for (const side of ["left", "right"] as const) {
    expect(lineSpans("foo", positions, side, dark)[0])
      .toEqual({ text: "foo", fg: dark.fg, bg: undefined });
  }
});

test("compact context preserves Rust-selected signature and changes with one gap per omitted run", () => {
  const file = createTestDiffFile();
  const lines = Array.from({ length: 12 }, (_, i) => `line ${i}`);
  file.diff.lhs_src = file.diff.rhs_src = { Text: lines.join("\n") };
  file.diff.lhs_positions = file.diff.rhs_positions = [];
  file.diff.aligned_rows = lines.map((_, i) => [i, i]);
  file.diff.hunks = [{novel_lhs: [], novel_rhs: [], lines: [[1,1], [2,2], [8,8], [9,9]]}];
  for (const layout of ["split", "unified"] as const) {
    const rows = rowsForFile(file, 0, layout, dark);
    expect(rows.slice(1).map(r => r.label ?? r.left?.lineNumber ?? r.cell?.oldLineNumber))
      .toEqual(["…", 2, 3, "…", 9, 10, "…"]);
    expect(rows.filter(r => r.hunkStart)).toHaveLength(1);
    expect(rowsForFile(file, 0, layout, dark, true)).toHaveLength(13);
  }
});

test("unified trusts diffr flags despite different source indentation", () => {
  const file = createTestDiffFile();
  file.diff.lhs_src = {Text: "  call();\n"};
  file.diff.rhs_src = {Text: "    call();\n"};
  file.diff.lhs_positions = file.diff.rhs_positions = [];
  file.diff.aligned_rows = [[0, 0]];
  file.diff.hunks = [{novel_lhs: [], novel_rhs: [], lines: [[0, 0]]}];
  const rows = rowsForFile(file, 0, "unified", dark);
  expect(rows).toHaveLength(2);
  expect(rows[1].cell).toMatchObject({kind: "context", sign: " ", oldLineNumber: 1, newLineNumber: 1});
  expect(rows[1].cell!.spans.map(s => s.text).join("")).toBe("    call();");
  for (const side of ["left", "right"] as const) {
    expect(copySelection([file], rows, {anchor: rows[1].key, end: rows[1].key, side}))
      .toBe(side === "left" ? "  call();" : "    call();");
  }
  // Only the side flagged novel may receive change styling.
  file.diff.hunks[0].novel_rhs = [0];
  const changed = rowsForFile(file, 0, "unified", dark).slice(1);
  expect(changed.map(r => r.cell!.kind)).toEqual(["context", "addition"]);
  file.diff.aligned_rows = [[0, null], [null, 0]];
  file.diff.hunks[0].novel_rhs = [];
  file.diff.hunks[0].lines = file.diff.aligned_rows;
  expect(rowsForFile(file, 0, "unified", dark).slice(1).map(r => r.cell!.kind))
    .toEqual(["context", "context"]);
});
