/** Project Rust full-file correspondence into Hunk terminal cells; never compute a second diff. */
import type { DiffFile, DiffResult, Highlight, MatchedPos } from "./wire";
import type {
  RenderSpan,
  SplitLineCell,
  UnifiedLineCell,
} from "../ui/diff/diffRowModel";
import { measureTextWidth } from "../ui/lib/text";
export type Layout = "split" | "unified";
export interface ViewerRow {
  key: string;
  fileIndex: number;
  hunkIndex?: number;
  hunkStart?: boolean;
  label?: string;
  left?: SplitLineCell;
  right?: SplitLineCell;
  cell?: UnifiedLineCell;
}
export interface Palette {
  bg: string;
  fg: string;
  muted: string;
  addition: string;
  deletion: string;
  addWord: string;
  deleteWord: string;
  keyword: string;
  type: string;
  comment: string;
}
export const dark: Palette = {
  bg: "#0d1117",
  fg: "#e6edf3",
  muted: "#8b949e",
  addition: "#12261e",
  deletion: "#301a20",
  addWord: "#24583a",
  deleteWord: "#74333c",
  keyword: "#ff7b72",
  type: "#79c0ff",
  comment: "#8b949e",
};
export const light: Palette = {
  bg: "#ffffff",
  fg: "#24292f",
  muted: "#57606a",
  addition: "#dafbe1",
  deletion: "#ffebe9",
  addWord: "#aceebb",
  deleteWord: "#ffcecb",
  keyword: "#cf222e",
  type: "#0550ae",
  comment: "#6e7781",
};
export const sourceLines = (text: string) =>
  text === "" ? [] : text.replace(/\n$/, "").split("\n");
function color(token: Highlight, theme: Palette) {
  if (token === "Delimiter") return theme.fg;
  const atom = token.Atom;
  return typeof atom === "object"
    ? theme.fg
    : atom === "Keyword"
      ? theme.keyword
      : atom === "Type"
        ? theme.type
        : atom === "Comment"
          ? theme.comment
          : theme.fg;
}
function byLine(positions: MatchedPos[]) {
  const result = new Map<number, MatchedPos[]>();
  for (const position of positions) {
    const list = result.get(position.pos.line) ?? [];
    list.push(position);
    result.set(position.pos.line, list);
  }
  return result;
}
/** Translate UTF-8 byte spans before expanding tabs into terminal cells. */
export function lineSpans(
  text: string,
  positions: MatchedPos[],
  side: "left" | "right",
  theme: Palette,
): RenderSpan[] {
  const bytes = new TextEncoder().encode(text),
    decoder = new TextDecoder("utf-8", { fatal: true });
  const spans: RenderSpan[] = [];
  let cursor = 0;
  for (const position of [...positions].sort(
    (a, b) => a.pos.start_col - b.pos.start_col,
  )) {
    const { start_col: start, end_col: end } = position.pos;
    if (start < cursor || end < start || end > bytes.length)
      throw new Error("Invalid or overlapping diffr token span");
    if (start > cursor)
      spans.push({
        text: decoder.decode(bytes.slice(cursor, start)),
        fg: theme.fg,
      });
    const kind = Object.values(position.kind)[0];
    const novel = "Novel" in position.kind || "NovelWord" in position.kind;
    spans.push({
      text: decoder.decode(bytes.slice(start, end)),
      fg: color(kind.highlight, theme),
      bg: novel
        ? side === "left"
          ? theme.deleteWord
          : theme.addWord
        : undefined,
    });
    cursor = end;
  }
  if (cursor < bytes.length)
    spans.push({ text: decoder.decode(bytes.slice(cursor)), fg: theme.fg });
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
/** Render the complete Rust alignment; hunks supply change flags and navigation only. */
export function rowsForFile(
  file: DiffFile,
  fileIndex: number,
  layout: Layout,
  theme: Palette,
  fullContext = false,
): ViewerRow[] {
  const d = file.diff;
  const rows: ViewerRow[] = [
    {
      key: `${fileIndex}:header`,
      fileIndex,
      label: file.file.new_path ?? file.file.old_path ?? d.display_path,
    },
  ];
  if (d.lhs_src === "Binary" || d.rhs_src === "Binary")
    return [
      ...rows,
      { key: `${fileIndex}:binary`, fileIndex, label: "Binary file" },
    ];
  const left = sourceLines(d.lhs_src.Text),
    right = sourceLines(d.rhs_src.Text);
  const positions = [byLine(d.lhs_positions), byLine(d.rhs_positions)];
  const caches = [
    new Map<number, RenderSpan[]>(),
    new Map<number, RenderSpan[]>(),
  ];
  const cell = (
    line: number | null,
    side: 0 | 1,
    novel: Set<number>,
  ): SplitLineCell => {
    if (line === null) return { kind: "empty", sign: " ", spans: [] };
    const text = (side ? right : left)[line];
    if (text === undefined)
      throw new Error(`diffr alignment references missing line ${line}`);
    let spans = caches[side].get(line);
    if (!spans) {
      spans = lineSpans(
        text,
        positions[side].get(line) ?? [],
        side ? "right" : "left",
        theme,
      );
      caches[side].set(line, spans);
    }
    const changed = novel.has(line);
    return {
      kind: changed ? (side ? "addition" : "deletion") : "context",
      sign: changed ? (side ? "+" : "-") : " ",
      lineNumber: line + 1,
      spans,
    };
  };
  const novelLeft = new Set(d.hunks.flatMap(h => h.novel_lhs));
  const novelRight = new Set(d.hunks.flatMap(h => h.novel_rhs));
  const hunkLeft = new Map<number, number>(), hunkRight = new Map<number, number>();
  for (const [index, hunk] of d.hunks.entries()) {
    for (const [l, r] of hunk.lines) {
      if (l !== null && !hunkLeft.has(l)) hunkLeft.set(l, index);
      if (r !== null && !hunkRight.has(r)) hunkRight.set(r, index);
    }
  }
  let pendingOld: ViewerRow[] = [],
    pendingNew: ViewerRow[] = [];
  const flush = () => {
    rows.push(...pendingOld, ...pendingNew);
    pendingOld = [];
    pendingNew = [];
  };
  for (const [l, r] of d.aligned_rows) {
    const hunkIndex = (l === null ? undefined : hunkLeft.get(l))
      ?? (r === null ? undefined : hunkRight.get(r));
    // Rust's hunk selection includes nearby lines and enclosing syntax context.
    // Keep both cells of a selected alignment row; never realign after hiding.
    if (!fullContext && hunkIndex === undefined) {
      flush();
      if (rows.at(-1)?.label !== "…") {
        rows.push({ key: `${fileIndex}:gap:${l ?? "_"}:${r ?? "_"}`, fileIndex, label: "…" });
      }
      continue;
    }
    const a = cell(l, 0, novelLeft),
      b = cell(r, 1, novelRight);
    const key = `${fileIndex}:${l ?? "_"}:${r ?? "_"}`;
    if (layout === "split")
      rows.push({ key, fileIndex, hunkIndex, left: a, right: b });
    else {
      // Correspondence and novelty come from diffr, including formatting-only changes.
      const shared =
        l !== null &&
        r !== null &&
        !novelLeft.has(l) &&
        !novelRight.has(r);
      if (shared) {
        flush();
        rows.push({
          key,
          fileIndex,
          hunkIndex,
          cell: {
            kind: "context",
            sign: " ",
            oldLineNumber: l + 1,
            newLineNumber: r + 1,
            spans: b.spans,
          },
        });
      } else {
        if (l !== null)
          pendingOld.push({
            key: `${key}:old`,
            fileIndex,
            hunkIndex,
            cell: {
              kind: a.kind === "deletion" ? "deletion" : "context",
              sign: a.sign,
              oldLineNumber: l + 1,
              spans: a.spans,
            },
          });
        if (r !== null)
          pendingNew.push({
            key: `${key}:new`,
            fileIndex,
            hunkIndex,
            cell: {
              kind: b.kind === "addition" ? "addition" : "context",
              sign: b.sign,
              newLineNumber: r + 1,
              spans: b.spans,
            },
          });
      }
    }
  }
  flush();
  const visited = new Set<number>();
  for (const row of rows) {
    if (row.hunkIndex !== undefined && !visited.has(row.hunkIndex)) {
      row.hunkStart = true;
      visited.add(row.hunkIndex);
    }
  }
  return rows;
}
