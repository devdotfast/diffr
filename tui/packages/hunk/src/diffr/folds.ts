/** Derive VS Code-style fold regions from diffr folds; collapsed state belongs to the viewer. */
import type { DiffResult } from "./wire";
export type Side = 0 | 1;
export interface FoldRegion {
  /** Shared by an unchanged fold and its opposite, so both sides collapse together. */
  id: string;
  side: Side;
  /** The header line, always visible, carrying the chevron and the placeholder. */
  startLine: number;
  /** The last line of the fold range; hidden only when nothing follows the range on it. */
  endLine: number;
  hideEnd: boolean;
  placeholder: string;
}
export interface RowFold {
  id: string;
  placeholder: string;
  collapsed: boolean;
}
export const sourceLines = (text: string) =>
  text === "" ? [] : text.replace(/\n$/, "").split("\n");
const encoder = new TextEncoder();
function regionsForSide(diff: DiffResult, side: Side): FoldRegion[] {
  const source = side ? diff.rhs_src : diff.lhs_src;
  const folds = side ? diff.rhs_folds : diff.lhs_folds;
  if (source === "Binary") return [];
  const lines = sourceLines(source.Text);
  const byStart = new Map<number, FoldRegion>();
  for (const fold of folds) {
    const { start, end } = fold.range;
    const endLine = lines[end.line];
    if (endLine === undefined)
      throw new Error(`diffr fold references missing line ${end.line}`);
    const hideEnd = encoder.encode(endLine.trimEnd()).length <= end.byte_column;
    const hiddenLines = end.line - start.line - (hideEnd ? 0 : 1);
    if (hiddenLines < 1) continue;
    const paired =
      fold.match_kind === "Novel"
        ? null
        : side
          ? fold.range
          : fold.match_kind.Unchanged.opposite;
    const id = paired
      ? `R${paired.start.line}:${paired.end.line}`
      : `${side ? "R" : "L"}${start.line}:${end.line}`;
    const region = { id, side, startLine: start.line, endLine: end.line, hideEnd,
      placeholder: fold.placeholder };
    // Like VS Code, one region per header line: keep the outermost.
    const existing = byStart.get(start.line);
    if (!existing || existing.endLine < end.line) byStart.set(start.line, region);
  }
  return [...byStart.values()].sort((a, b) => a.startLine - b.startLine);
}
export function foldRegions(diff: DiffResult): FoldRegion[] {
  return [...regionsForSide(diff, 0), ...regionsForSide(diff, 1)];
}
/** Regions strictly inside another on the same side, for Alt-click recursive folding. */
export function nestedRegions(regions: FoldRegion[], outer: FoldRegion) {
  return regions.filter(
    (r) => r.side === outer.side && r.startLine > outer.startLine && r.endLine <= outer.endLine,
  );
}
/** First aligned row showing each source line, per side. */
function rowsOfLines(alignedRows: DiffResult["aligned_rows"]) {
  const rowOfLine = [new Map<number, number>(), new Map<number, number>()];
  for (const [index, pair] of alignedRows.entries())
    for (const side of [0, 1] as const) {
      const line = pair[side];
      if (line !== null && !rowOfLine[side].has(line)) rowOfLine[side].set(line, index);
    }
  return rowOfLine;
}
/** Source lines hidden per side. Each side folds independently; alignment is never recomputed. */
export function hiddenLines(
  alignedRows: DiffResult["aligned_rows"],
  regions: FoldRegion[],
  collapsed: ReadonlySet<string>,
  headerShown: (rowIndex: number) => boolean,
): [Set<number>, Set<number>] {
  const rowOfLine = rowsOfLines(alignedRows);
  const hidden: [Set<number>, Set<number>] = [new Set(), new Set()];
  for (const region of regions) {
    if (!collapsed.has(region.id)) continue;
    const startRow = rowOfLine[region.side].get(region.startLine);
    if (startRow === undefined)
      throw new Error(`diffr fold at line ${region.startLine} is not aligned`);
    if (!headerShown(startRow)) continue;
    const last = region.endLine - (region.hideEnd ? 0 : 1);
    for (let line = region.startLine + 1; line <= last; line++) hidden[region.side].add(line);
  }
  return hidden;
}
/** Header rows keyed by aligned-row index, one per side. */
export function foldHeaders(
  alignedRows: DiffResult["aligned_rows"],
  regions: FoldRegion[],
  collapsed: ReadonlySet<string>,
): [Map<number, RowFold>, Map<number, RowFold>] {
  const rowOfLine = rowsOfLines(alignedRows);
  const headers: [Map<number, RowFold>, Map<number, RowFold>] = [new Map(), new Map()];
  for (const region of regions) {
    const row = rowOfLine[region.side].get(region.startLine);
    if (row === undefined)
      throw new Error(`diffr fold at line ${region.startLine} is not aligned`);
    headers[region.side].set(row, {
      id: region.id,
      placeholder: region.placeholder,
      collapsed: collapsed.has(region.id),
    });
  }
  return headers;
}
