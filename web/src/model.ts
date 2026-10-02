/**
 * The rows a file paints, from diffr's per-side region trees. The zip is the TUI's (rows.ts): leaves
 * pair on their alignment ids, folds hide lines and stand in as one row, and nothing realigns when a
 * fold toggles. Unlike rows.ts this keeps line indices only; spans are built when a row is painted,
 * so a 50k-line file costs nothing until it scrolls into view.
 */
import {
  collapsedFolds, flatten, foldHeaders, foldTint, hiddenLines, novelLeaves, pairedIds,
  type Fold, type FoldTint, type Leaf, type RowFold,
} from "../../tui/packages/hunk/src/diffr/regions";
import type { Span, TextDiff } from "../../tui/packages/hunk/src/diffr/wire";
import type { Lines } from "./lines";
import type { Syntax } from "./syntax";

export type Layout = "split" | "unified";
export type LineKind = "context" | "change-deletion" | "change-addition";

/** One side of a split row, or the one line of a unified row. */
export interface Cell {
  side: 0 | 1;
  line: number;
  kind: LineKind;
  /** This line opens a region the reader can fold. */
  fold?: RowFold;
  /** Change spans diffr found on this line. */
  changed: Span[];
  /** Unified layout: the other side's line number for an unchanged line. */
  alt?: number;
}

export type Row =
  | { type: "line"; key: string; left?: Cell; right?: Cell }
  | { type: "line"; key: string; cell: Cell }
  /** A collapsed region: one row where its first line would have been. */
  | { type: "fold"; key: string; id: number; label: string; tint: FoldTint; lines: number; extra: string[] };

/** Everything about a file's diff that does not depend on what is folded. */
export interface Prepared {
  diff: TextDiff;
  texts: [Lines, Lines];
  syntax: [Syntax | undefined, Syntax | undefined];
  leaves: readonly [Leaf[], Leaf[]];
  folds: readonly [Fold[], Fold[]];
  paired: readonly [Set<number>, Set<number>];
  novel: Set<Leaf>;
  /** Right leaf index by alignment id, for the zip. */
  rightIndex: Map<number, number>;
}

/** `lines` and `syntax` come split and packed from the worker, which empties them out of `diff`. */
export function prepare(diff: TextDiff, lines: Prepared["texts"], syntax: Prepared["syntax"]): Prepared {
  const { leaves, folds } = flatten(diff);
  return {
    diff,
    texts: lines,
    syntax,
    leaves,
    folds,
    paired: pairedIds(diff),
    novel: novelLeaves(leaves),
    rightIndex: new Map(leaves[1].map((leaf, index) => [leaf.alignmentId, index])),
  };
}

/** Lines a collapsed region hides on the side that hides more, for its label. */
function hiddenCount(region: Leaf | Fold) {
  return "endLine" in region ? region.endLine - region.startLine : region.lastHidden - region.startLine + 1;
}

const plural = (n: number, word: string) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;

export function buildRows(p: Prepared, layout: Layout, collapsed: ReadonlySet<number>): Row[] {
  const { leaves, folds, paired, novel } = p;
  const hidden = [hiddenLines(folds[0], collapsed), hiddenLines(folds[1], collapsed)];
  const bands = [collapsedFolds(folds[0], collapsed), collapsedFolds(folds[1], collapsed)];
  const headers = [foldHeaders(folds[0], leaves[0], collapsed, paired[0]), foldHeaders(folds[1], leaves[1], collapsed, paired[1])];
  const rows: Row[] = [];
  let pendingOld: Row[] = [], pendingNew: Row[] = [];
  const flush = () => {
    if (pendingOld.length) rows.push(...pendingOld);
    if (pendingNew.length) rows.push(...pendingNew);
    pendingOld = [];
    pendingNew = [];
  };

  const cell = (leaf: Leaf, line: number, side: 0 | 1): Cell => ({
    side,
    line,
    kind: novel.has(leaf) ? (side ? "change-addition" : "change-deletion") : "context",
    fold: headers[side].get(line),
    // A leaf with no counterpart is new from end to end; its background says so without word marks.
    changed: paired[side].has(leaf.id) ? leaf.changed.get(line) ?? [] : [],
  });

  const foldRow = (key: string, regions: [Leaf | Fold | null, Leaf | Fold | null], label: (r: Leaf | Fold) => string) => {
    flush();
    const anchor = (regions[0] ?? regions[1])!;
    const text = (regions[0] && label(regions[0])) || (regions[1] && label(regions[1])) || "";
    const lines = Math.max(...regions.map((r) => (r ? hiddenCount(r) : 0)));
    const [first, ...extra] = text.split("\n");
    rows.push({
      type: "fold", key, id: anchor.foldStateId,
      // Context gaps arrive labelled by the context plugin; anything unlabelled says what it hides.
      label: first || plural(lines, "line"),
      tint: foldTint(anchor.id, anchor.side, paired[anchor.side]),
      lines, extra,
    });
  };

  const emit = (l: number | null, r: number | null, left: Leaf | null, right: Leaf | null) => {
    if (l !== null && hidden[0].has(l)) l = null;
    if (r !== null && hidden[1].has(r)) r = null;
    if (l === null && r === null) return;
    const a = l !== null ? cell(left!, l, 0) : undefined;
    const b = r !== null ? cell(right!, r, 1) : undefined;
    const key = `${l ?? "_"}:${r ?? "_"}`;
    if (layout === "split") {
      rows.push({ type: "line", key, left: a, right: b });
      return;
    }
    // Unified: a pair of unchanged lines is one row; changes queue removals before additions.
    if (a && b && a.kind === "context" && b.kind === "context") {
      flush();
      rows.push({ type: "line", key, cell: { ...b, fold: b.fold ?? a.fold, alt: a.line } });
      return;
    }
    // One side of a pair lost nothing: the other line says everything, so it stands alone.
    if (a && b && a.kind === "context" && !a.changed.length) {
      pendingNew.push({ type: "line", key: `${key}:n`, cell: { ...b, fold: b.fold ?? a.fold, alt: a.line } });
      return;
    }
    if (a && b && b.kind === "context" && !b.changed.length) {
      pendingOld.push({ type: "line", key: `${key}:o`, cell: { ...a, fold: a.fold ?? b.fold, alt: b.line } });
      return;
    }
    if (a) pendingOld.push({ type: "line", key: `${key}:o`, cell: a });
    if (b) pendingNew.push({ type: "line", key: `${key}:n`, cell: b });
  };

  const leafRows = (left: Leaf | null, right: Leaf | null) => {
    const anchor = left ?? right;
    if (!anchor) return;
    // Every leaf is split at its folds' edges, so a collapsed fold's row goes before the leaf
    // that would have started it.
    const lb = left ? bands[0].get(left.startLine) ?? null : null;
    const rb = right ? bands[1].get(right.startLine) ?? null : null;
    if (lb || rb) foldRow(`f${(lb ?? rb)!.foldStateId}`, [lb, rb], (f) => f.label);
    if (collapsed.has(anchor.foldStateId)) {
      if (!hidden[anchor.side].has(anchor.startLine)) foldRow(`g${anchor.foldStateId}`, [left, right], (l) => l.label);
      return;
    }
    const ln = left ? left.endLine - left.startLine : 0, rn = right ? right.endLine - right.startLine : 0;
    for (let i = 0, n = Math.max(ln, rn); i < n; i++)
      emit(i < ln ? left!.startLine + i : null, i < rn ? right!.startLine + i : null, left, right);
  };

  let cursor = 0;
  const flushRight = (until: number) => {
    for (; cursor < until; cursor++) leafRows(null, leaves[1][cursor]!);
  };
  for (const left of leaves[0]) {
    const partner = p.rightIndex.get(left.alignmentId);
    if (partner === undefined) {
      leafRows(left, null);
      continue;
    }
    flushRight(partner);
    leafRows(left, leaves[1][partner]!);
    cursor = partner + 1;
  }
  flushRight(leaves[1].length);
  flush();
  return rows;
}
