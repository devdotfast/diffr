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
  expect(geometry.rows[3].height).toBeGreaterThan(1);
  expect(geometry.rows[3].height).toBe(
    Math.max(geometry.rows[3].left.length, geometry.rows[3].right.length),
  );
  expect(visibleRows(geometry, 0, 2).length).toBe(2);
});
test("selection copies one source side and excludes padding and added lines", () => {
  const file = createTestDiffFile(),
    rows = rowsForFile(file, 0, "split", dark);
  expect(
    copySelection([file], rows, {
      anchor: rows[2].key,
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
  const geometry = measureRows(
    rowsForFile(file, 0, "split", dark),
    120,
    false,
    0,
  );
  expect(visibleRows(geometry, 10000, 40)).toHaveLength(40);
});
