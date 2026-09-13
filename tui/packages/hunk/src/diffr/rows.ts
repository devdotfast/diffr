/** Project diffr's per-side region trees into terminal cells by zipping leaves on their ids. */
import type { DiffFile, Span, SyntaxSpan } from "./wire";
import { filePath } from "./wire";
import type { RenderSpan, SplitLineCell, UnifiedLineCell } from "../ui/diff/diffRowModel";
import { measureTextWidth } from "../ui/lib/text";
import { flatten, foldHeaders, hiddenLines, leafLabel, novelLeaves, sourceLines, type Leaf, type RowFold } from "./regions";
import { loadBundledTheme, type Palette } from "./theme";
export { sourceLines };
export type Layout = "split" | "unified";
export interface ViewerRow {
  key: string;
  fileIndex: number;
  /** First row of a run of changed rows, for `[` and `]`. */
  hunkStart?: boolean;
  label?: string;
  /** A hidden-file placeholder: "Load diff" reveals the file. */
  loadDiff?: boolean;
  left?: SplitLineCell;
  right?: SplitLineCell;
  cell?: UnifiedLineCell;
}
export type { Palette } from "./theme";
/** The bundled defaults, for tests and the settings screen. */
export const dark: Palette = loadBundledTheme("default-dark");
export const light: Palette = loadBundledTheme("default-light");
/** Foreground for a tree-sitter capture: the theme's scope, its parents, else plain text. */
export function captureColor(capture: string, theme: Palette): string {
  return theme.syntax(capture) ?? theme.fg;
}
/** Colour a line from byte-addressed syntax and change spans, then expand tabs into cells. */
export function lineSpans(
  text: string,
  syntax: SyntaxSpan[],
  changed: Span[],
  side: "left" | "right",
  theme: Palette,
): RenderSpan[] {
  const bytes = new TextEncoder().encode(text),
    decoder = new TextDecoder("utf-8", { fatal: true });
  const bounds = new Set([0, bytes.length]);
  for (const span of [...syntax, ...changed]) {
    if (span.start_column > span.end_column || span.end_column > bytes.length)
      throw new Error("Invalid diffr span");
    bounds.add(span.start_column);
    bounds.add(span.end_column);
  }
  const sorted = [...bounds].sort((a, b) => a - b);
  const wordBg = side === "left" ? theme.deleteWord : theme.addWord;
  const spans: RenderSpan[] = [];
  for (let i = 0; i + 1 < sorted.length; i++) {
    const start = sorted[i], end = sorted[i + 1];
    // Innermost syntax capture wins, so pick the narrowest span covering this segment.
    const capture = syntax
      .filter((s) => s.start_column <= start && s.end_column >= end)
      .sort((a, b) => a.end_column - a.start_column - (b.end_column - b.start_column))[0];
    const emphasized = changed.some((s) => s.start_column <= start && s.end_column >= end);
    spans.push({
      text: decoder.decode(bytes.slice(start, end)),
      fg: capture ? captureColor(capture.capture, theme) : theme.fg,
      bg: emphasized ? wordBg : undefined,
    });
  }
  let column = 0;
  return spans.map((span) => ({
    ...span,
    text: span.text
      .split("\t")
      .map((part, index) => {
        const padding = index ? " ".repeat(4 - (column % 4)) : "";
        column += padding.length + measureTextWidth(part);
        return padding + part;
      })
      .join(""),
  }));
}
function byLine(spans: SyntaxSpan[]) {
  const result = new Map<number, SyntaxSpan[]>();
  for (const span of spans) result.set(span.line, [...(result.get(span.line) ?? []), span]);
  return result;
}
const indentOf = (text: string) => text.match(/^\s*/)![0];
/** Rows for a hidden file: GitHub's "Load diff" placeholder under the header. */
export function placeholderRows(fileIndex: number, label: string): ViewerRow[] {
  return [
    { key: `${fileIndex}:load`, fileIndex, label: "Load diff", loadDiff: true },
    { key: `${fileIndex}:why`, fileIndex, label: label || "Hidden by default" },
  ];
}
/** Render both sides by zipping leaves on their ids; folding never realigns. */
export function rowsForFile(
  file: DiffFile,
  fileIndex: number,
  layout: Layout,
  theme: Palette,
  collapsed: ReadonlySet<number> = new Set(),
): ViewerRow[] {
  const d = file.diff;
  const rows: ViewerRow[] = [{ key: `${fileIndex}:header`, fileIndex, label: filePath(file.file) }];
  if (d.type === "binary")
    return [...rows, { key: `${fileIndex}:binary`, fileIndex, label: "Binary file" }];
  const texts = [sourceLines(d.lhs?.text ?? ""), sourceLines(d.rhs?.text ?? "")];
  const syntax = [byLine(d.lhs?.syntax ?? []), byLine(d.rhs?.syntax ?? [])];
  const { leaves, folds } = flatten(d);
  const hidden = [hiddenLines(folds[0], collapsed), hiddenLines(folds[1], collapsed)];
  const headers = [foldHeaders(folds[0], leaves[0], collapsed), foldHeaders(folds[1], leaves[1], collapsed)];
  const caches = [new Map<number, RenderSpan[]>(), new Map<number, RenderSpan[]>()];
  // Every line of a novel leaf is tinted; the spans inside get the darker word tint on top.
  const novelSet = novelLeaves(leaves);
  const novel = (leaf: Leaf) => novelSet.has(leaf);
  const cell = (leaf: Leaf | null, line: number | null, side: 0 | 1): SplitLineCell => {
    if (line === null || leaf === null) return { kind: "empty", sign: " ", spans: [] };
    const text = texts[side][line];
    if (text === undefined) throw new Error(`diffr region references missing line ${line}`);
    let spans = caches[side].get(line);
    if (!spans) {
      spans = lineSpans(text, syntax[side].get(line) ?? [], leaf.changed.get(line) ?? [],
        side ? "right" : "left", theme);
      caches[side].set(line, spans);
    }
    const changed = novel(leaf);
    return {
      kind: changed ? (side ? "addition" : "deletion") : "context",
      sign: changed ? (side ? "+" : "-") : " ",
      lineNumber: line + 1,
      spans,
      fold: headers[side].get(line),
    };
  };
  let pendingOld: ViewerRow[] = [], pendingNew: ViewerRow[] = [];
  const flush = () => {
    rows.push(...pendingOld, ...pendingNew);
    pendingOld = [];
    pendingNew = [];
  };
  // A collapsed leaf is one fold row: chevron and label, no line number, same toggle as a body fold.
  const collapsedLeaf = (left: Leaf | null, right: Leaf | null) => {
    flush();
    const anchor = (left ?? right)!;
    const folded = (leaf: Leaf | null): SplitLineCell => leaf
      ? { kind: "context", sign: " ", spans: [], fold: { id: leaf.foldStateId, label: leafLabel(leaf), collapsed: true } }
      : { kind: "empty", sign: " ", spans: [] };
    const key = `${fileIndex}:gap:${anchor.foldStateId}`;
    if (layout === "split") rows.push({ key, fileIndex, left: folded(left), right: folded(right) });
    else rows.push({ key, fileIndex, cell: { kind: "context", sign: " ", spans: [], fold: folded(anchor).fold } });
  };
  const emit = (l: number | null, r: number | null, left: Leaf | null, right: Leaf | null) => {
    if (l !== null && hidden[0].has(l)) l = null;
    if (r !== null && hidden[1].has(r)) r = null;
    if (l === null && r === null) return;
    const a = cell(left, l, 0), b = cell(right, r, 1);
    const key = `${fileIndex}:${l ?? "_"}:${r ?? "_"}`;
    if (layout === "split") {
      rows.push({ key, fileIndex, left: a, right: b });
      return;
    }
    // Correspondence and novelty come from diffr, including formatting-only changes.
    if (l !== null && r !== null && a.kind === "context" && b.kind === "context") {
      flush();
      rows.push({ key, fileIndex, cell: { kind: "context", sign: " ", oldLineNumber: l + 1,
        newLineNumber: r + 1, fold: b.fold ?? a.fold, spans: b.spans } });
      return;
    }
    if (l !== null)
      pendingOld.push({ key: `${key}:old`, fileIndex, cell: { kind: a.kind === "deletion" ? "deletion" : "context",
        sign: a.sign, oldLineNumber: l + 1, fold: a.fold, spans: a.spans } });
    if (r !== null)
      pendingNew.push({ key: `${key}:new`, fileIndex, cell: { kind: b.kind === "addition" ? "addition" : "context",
        sign: b.sign, newLineNumber: r + 1, fold: b.fold, spans: b.spans } });
  };
  const leafRows = (left: Leaf | null, right: Leaf | null) => {
    const anchor = left ?? right;
    if (!anchor) return;
    if (collapsed.has(anchor.foldStateId)) {
      if (!hidden[anchor.side].has(anchor.startLine)) collapsedLeaf(left, right);
      return;
    }
    const length = Math.max(left ? left.endLine - left.startLine : 0, right ? right.endLine - right.startLine : 0);
    for (let i = 0; i < length; i++)
      emit(left && i < left.endLine - left.startLine ? left.startLine + i : null,
        right && i < right.endLine - right.startLine ? right.startLine + i : null, left, right);
  };
  // Zip: walk the left leaves; a partner ahead on the right flushes what precedes it as right-only.
  const rightIndex = new Map(leaves[1].map((leaf, index) => [leaf.alignmentId, index]));
  let cursor = 0;
  const flushRight = (until: number) => {
    for (; cursor < until; cursor++) leafRows(null, leaves[1][cursor]);
  };
  for (const left of leaves[0]) {
    const partner = rightIndex.get(left.alignmentId);
    if (partner === undefined || partner < cursor) {
      // Unpaired, or a move whose partner was already shown: one-sided rows.
      leafRows(left, null);
      continue;
    }
    flushRight(partner);
    leafRows(left, leaves[1][partner]);
    cursor = partner + 1;
  }
  flushRight(leaves[1].length);
  flush();
  return withFoldLabels(markHunks(rows), texts, theme);
}
function markHunks(rows: ViewerRow[]): ViewerRow[] {
  let inHunk = false;
  for (const row of rows) {
    const changed = row.cell
      ? row.cell.kind !== "context"
      : row.left !== undefined && (row.left.kind !== "context" || row.right!.kind !== "context");
    if (changed && !inHunk) row.hunkStart = true;
    inHunk = changed;
  }
  return rows;
}
/** A collapsed fold with a multi-line label shows the label under its header, inside the fold tint. */
function withFoldLabels(rows: ViewerRow[], texts: string[][], theme: Palette): ViewerRow[] {
  const result: ViewerRow[] = [];
  const labelLines = (fold: RowFold | undefined) =>
    fold?.collapsed && fold.label.includes("\n") ? fold.label.split("\n") : [];
  const labelCell = (text: string | undefined, kind: SplitLineCell["kind"], indent: string): SplitLineCell =>
    text === undefined
      ? { kind: "empty", sign: " ", spans: [] }
      : { kind, sign: " ", foldLabel: true, spans: [{ text: indent + text, fg: theme.foldPlaceholder }] };
  for (const row of rows) {
    result.push(row);
    if (row.cell) {
      const lines = labelLines(row.cell.fold);
      const line = row.cell.newLineNumber ?? row.cell.oldLineNumber;
      const indent = indentOf(texts[row.cell.newLineNumber === undefined ? 0 : 1][(line ?? 1) - 1] ?? "") + "    ";
      lines.forEach((text, i) => result.push({ key: `${row.key}:label:${i}`, fileIndex: row.fileIndex,
        cell: { ...labelCell(text, "context", indent), oldLineNumber: undefined, newLineNumber: undefined } as UnifiedLineCell }));
      continue;
    }
    if (!row.left || !row.right) continue;
    const left = labelLines(row.left.fold), right = labelLines(row.right.fold);
    const indents = [
      indentOf(texts[0][(row.left.lineNumber ?? 1) - 1] ?? "") + "    ",
      indentOf(texts[1][(row.right.lineNumber ?? 1) - 1] ?? "") + "    ",
    ];
    for (let i = 0; i < Math.max(left.length, right.length); i++)
      result.push({ key: `${row.key}:label:${i}`, fileIndex: row.fileIndex,
        left: labelCell(left[i], row.left.kind === "empty" ? "context" : row.left.kind, indents[0]),
        right: labelCell(right[i], row.right.kind === "empty" ? "context" : row.right.kind, indents[1]) });
  }
  return result;
}
