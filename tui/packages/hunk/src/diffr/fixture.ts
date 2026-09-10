/** Provide a hand-authored source correspondence for renderer and protocol tests. */
import type { DiffFile } from "./wire";
export function createTestDiffFile(): DiffFile {
  return {
    type: "file",
    file: {
      old_path: "demo.ts",
      new_path: "demo.ts",
      class: null,
      status: "modified",
    },
    diff: {
      display_path: "demo.ts",
      extra_info: null,
      file_format: { SupportedLanguage: "TypeScript" },
      lhs_src: { Text: 'start();\nsend("old");\nfinish();\n' },
      rhs_src: { Text: 'start();\nsend("new");\nextra();\nfinish();\n' },
      lhs_positions: [
        {
          pos: { line: 1, start_col: 6, end_col: 9 },
          kind: {
            NovelWord: { highlight: { Atom: { String: "StringLiteral" } } },
          },
        },
      ],
      rhs_positions: [
        {
          pos: { line: 1, start_col: 6, end_col: 9 },
          kind: {
            NovelWord: { highlight: { Atom: { String: "StringLiteral" } } },
          },
        },
      ],
      hunks: [
        {
          novel_lhs: [1],
          novel_rhs: [1, 2],
          lines: [
            [0, 0],
            [1, 1],
            [null, 2],
            [2, 3],
          ],
        },
      ],
      lhs_folds: [],
      rhs_folds: [],
      has_byte_changes: [3, 4],
      has_syntactic_changes: true,
    },
  };
}
