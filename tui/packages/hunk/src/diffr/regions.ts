/** Flatten diffr's per-side region trees; collapsed state belongs to the viewer. */
import type { Region, Source, Span, TextDiff } from "./wire";
export type Side = 0 | 1;
/** A leaf tiles its side; the same alignmentId on the other side is its counterpart. */
export interface Leaf {
  /** Pairs this leaf with its counterpart across sides; the row zip keys on it. */
  alignmentId: number;
  /** Regions sharing it toggle together; collapse state keys on it. */
  foldStateId: number;
  side: Side;
  startLine: number;
  /** Exclusive. */
  endLine: number;
  changed: Map<number, Span[]>;
  tags: string[];
  collapsed: boolean;
  label: string;
}
export interface Fold {
  alignmentId: number;
  foldStateId: number;
  side: Side;
  /** The header line, always visible, carrying the chevron and the label. */
  headerLine: number;
  /** Last hidden line, inclusive; the closing delimiter stays visible when text follows the range. */
  lastHidden: number;
  label: string;
  tags: string[];
  collapsed: boolean;
  /** Ids of folds nested inside, for recursive fold commands. */
  nested: number[];
}
export interface RowFold {
  /** The fold-state id: what toggling this header toggles. */
  id: number;
  label: string;
  collapsed: boolean;
}
export const sourceLines = (text: string) =>
  text === "" ? [] : text.replace(/\n$/, "").split("\n");
const encoder = new TextEncoder();
/** Lines a leaf covers, half-open: an end at column zero does not touch its end line. */
export function leafLines(region: Region): [number, number] {
  return [region.start.line, region.end.column === 0 ? region.end.line : region.end.line + 1];
}
export function flattenSide(source: Source, side: Side): { leaves: Leaf[]; folds: Fold[] } {
  const lines = sourceLines(source.text);
  const leaves: Leaf[] = [];
  const folds: Fold[] = [];
  const visit = (region: Region, ancestors: Fold[]) => {
    if (region.kind === "leaf") {
      const [startLine, endLine] = leafLines(region);
      const changed = new Map<number, Span[]>();
      for (const span of region.changed) changed.set(span.line, [...(changed.get(span.line) ?? []), span]);
      leaves.push({ alignmentId: region.alignment_id, foldStateId: region.fold_state_id, side, startLine, endLine,
        changed, tags: region.tags,
        collapsed: region.visibility.collapsed, label: region.visibility.label });
      return;
    }
    const { start, end } = region;
    const endText = lines[end.line];
    if (endText === undefined) throw new Error(`diffr fold references missing line ${end.line}`);
    // An end at column 0 does not touch its line; otherwise the end line is hidden when the
    // range covers all of its text.
    const hideEnd = end.column > 0 && encoder.encode(endText.trimEnd()).length <= end.column;
    const fold: Fold = { alignmentId: region.alignment_id, foldStateId: region.fold_state_id, side,
      headerLine: start.line,
      lastHidden: end.line - (hideEnd ? 0 : 1), label: region.visibility.label, tags: region.tags,
      collapsed: region.visibility.collapsed, nested: [] };
    for (const ancestor of ancestors) ancestor.nested.push(fold.foldStateId);
    folds.push(fold);
    for (const child of region.children) visit(child, [...ancestors, fold]);
  };
  for (const region of source.regions) visit(region, []);
  return { leaves, folds };
}
export function flatten(diff: TextDiff) {
  const lhs = diff.lhs ? flattenSide(diff.lhs, 0) : { leaves: [], folds: [] };
  const rhs = diff.rhs ? flattenSide(diff.rhs, 1) : { leaves: [], folds: [] };
  return { leaves: [lhs.leaves, rhs.leaves] as const, folds: [lhs.folds, rhs.folds] as const };
}
/** A leaf is novel when it carries change spans or has no counterpart on the other side. */
export function novelLeaves(leaves: readonly [Leaf[], Leaf[]]): Set<Leaf> {
  const ids = [new Set(leaves[0].map((l) => l.alignmentId)), new Set(leaves[1].map((l) => l.alignmentId))];
  const novel = new Set<Leaf>();
  for (const side of [0, 1] as const)
    for (const leaf of leaves[side])
      if (leaf.changed.size > 0 || !ids[side ? 0 : 1].has(leaf.alignmentId)) novel.add(leaf);
  return novel;
}
/** Fold-state ids diffr asks to start collapsed: context gaps and folded bodies, on either side. */
export function defaultCollapsed(diff: TextDiff): Set<number> {
  const ids = new Set<number>();
  const { leaves, folds } = flatten(diff);
  for (const item of [...leaves.flat(), ...folds.flat()]) if (item.collapsed) ids.add(item.foldStateId);
  return ids;
}
/** Every collapsible id on either side, folds and foldable leaves alike, for fold-all commands. */
export function foldIds(diff: TextDiff): number[] {
  const { folds, leaves } = flatten(diff);
  return [...new Set([...folds.flat().map((fold) => fold.foldStateId), ...leaves.flat().filter(foldableLeaf).map((l) => l.foldStateId)])];
}
/** Ids nested inside a collapsible region, for recursive fold commands. */
export function nestedIds(diff: TextDiff, id: number): number[] {
  const fold = flatten(diff).folds.flat().find((f) => f.foldStateId === id);
  if (fold) return fold.nested;
  if (!flatten(diff).leaves.flat().some((leaf) => leaf.foldStateId === id)) throw new Error(`Unknown region ${id}`);
  return [];
}
/** Context-gap leaves: unchanged runs diffr trimmed to N lines around changes. */
export function gapIds(diff: TextDiff): number[] {
  return [...new Set(flatten(diff).leaves.flat().filter((leaf) => leaf.tags.includes("unchanged")).map((l) => l.foldStateId))];
}
/** Source lines hidden on one side by collapsed folds whose header is visible. */
export function hiddenLines(folds: Fold[], collapsed: ReadonlySet<number>): Set<number> {
  const hidden = new Set<number>();
  // Outer folds first: a fold whose header is already hidden hides nothing of its own.
  const ordered = [...folds].sort((a, b) => a.headerLine - b.headerLine || b.lastHidden - a.lastHidden);
  for (const fold of ordered) {
    if (!collapsed.has(fold.foldStateId) || hidden.has(fold.headerLine)) continue;
    for (let line = fold.headerLine + 1; line <= fold.lastHidden; line++) hidden.add(line);
  }
  return hidden;
}
/** A leaf collapses like a fold when diffr labelled it or tagged it as a context gap. */
export const foldableLeaf = (leaf: Leaf) => leaf.label !== "" || leaf.tags.includes("unchanged");
export const leafLabel = (leaf: Leaf) =>
  leaf.label || `${leaf.endLine - leaf.startLine} unchanged lines`;
/** Fold headers keyed by source line, one per side; a fold wins the header line of its first leaf. */
export function foldHeaders(folds: Fold[], leaves: Leaf[], collapsed: ReadonlySet<number>): Map<number, RowFold> {
  const headers = new Map<number, RowFold>();
  for (const leaf of leaves)
    if (foldableLeaf(leaf))
      headers.set(leaf.startLine, { id: leaf.foldStateId, label: leafLabel(leaf), collapsed: collapsed.has(leaf.foldStateId) });
  for (const fold of folds) {
    const existing = headers.get(fold.headerLine);
    const existingFold = existing && folds.find((f) => f.foldStateId === existing.id);
    if (existingFold && existingFold.lastHidden >= fold.lastHidden) continue;
    headers.set(fold.headerLine, { id: fold.foldStateId, label: fold.label, collapsed: collapsed.has(fold.foldStateId) });
  }
  return headers;
}
