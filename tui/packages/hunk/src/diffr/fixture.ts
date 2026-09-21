/** Provide a hand-authored wire v3 file record for renderer and protocol tests. */
import type { DiffEvent, DiffFile, FileChange, FoldRegion, LeafRegion, Region, Span } from "./wire";
const pos = (line: number, column = 0) => ({ line, column });
/**
 * A leaf whose `id`, `fold_state_id` and `alignment_id` are all `id`. Tests pair two leaves by
 * giving them one number on both sides, so the pair repeats its `id` across sides, which diffr
 * never does; the viewer keys identity per side, and the tests that tell the ids apart set them.
 */
export function leaf(id: number, start: number, end: number, changed: Span[] = []): LeafRegion {
  return { id, fold_state_id: id, alignment_id: id, start: pos(start), end: pos(end), tags: [],
    visibility: { collapsed: false, label: "" }, kind: "leaf", changed, children: [] };
}
export function fold(
  id: number,
  start: [number, number],
  end: [number, number],
  children: Region[],
  label = "Body",
  tags = ["deleted-bodies:function"],
  collapsed = false,
): FoldRegion {
  return { id, fold_state_id: id, start: pos(...start), end: pos(...end), tags,
    visibility: { collapsed, label }, kind: "fold", changed: [], children };
}
export const line = (line: number, start_column: number, end_column: number): Span =>
  ({ line, start_column, end_column });
export function createTestDiffFile(): DiffFile {
  return {
    type: "file",
    file: {
      lhs: { path: "demo.ts", oid: "1111111", mode: "100644" },
      rhs: { path: "demo.ts", oid: "2222222", mode: "100644" },
    },
    visibility: { collapsed: false, label: "" },
    diff: {
      type: "text",
      lhs: {
        text: 'start();\nsend("old");\nfinish();\n',
        syntax: [
          { line: 1, start_column: 0, end_column: 4, capture: "function.call" },
          { line: 1, start_column: 5, end_column: 10, capture: "string" },
        ],
        regions: [leaf(1, 0, 1), leaf(2, 1, 2, [line(1, 6, 9)]), leaf(4, 2, 3)],
      },
      rhs: {
        text: 'start();\nsend("new");\nextra();\nfinish();\n',
        syntax: [
          { line: 1, start_column: 0, end_column: 4, capture: "function.call" },
          { line: 1, start_column: 5, end_column: 10, capture: "string" },
        ],
        regions: [
          leaf(1, 0, 1),
          leaf(2, 1, 2, [line(1, 6, 9)]),
          leaf(3, 2, 3, [line(2, 0, 8)]),
          leaf(4, 3, 4),
        ],
      },
      stats: { textual: { added: 2, removed: 1 }, visible: { added: 2, removed: 1 } },
    },
  };
}
/** The manifest entry a file record answers. */
export const manifestEntry = (file: DiffFile): FileChange => ({ file: file.file, status: "modified", tags: [] });
/** A `start` record whose manifest lists these files. */
export const startFor = (files: DiffFile[]): DiffEvent =>
  ({ type: "start", version: 3, lhs: { type: "index" }, rhs: { type: "working_tree" }, files: files.map(manifestEntry) });
/** Replace both sides with identical numbered lines paired in one leaf. */
export function withIdenticalLines(file: DiffFile, count: number): DiffFile {
  const lines = Array.from({ length: count }, (_, i) => `line ${i}`);
  const text = lines.join("\n");
  const source = () => ({ text, syntax: [], regions: [leaf(1, 0, count)] });
  if (file.diff.type !== "text") throw new Error("fixture is not a text diff");
  file.diff.lhs = source();
  file.diff.rhs = source();
  return file;
}
