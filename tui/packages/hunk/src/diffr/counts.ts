/** Changed-line counts as shown: only lines on screen, so fold and file state changes them. */
import { defaultCollapsed, flatten, hiddenLines, novelLeaves } from "./regions";
import type { DiffFile, LineCounts } from "./wire";
export type { LineCounts };
export const zero: LineCounts = { added: 0, removed: 0 };
export const add = (a: LineCounts, b: LineCounts): LineCounts => ({ added: a.added + b.added, removed: a.removed + b.removed });
const subtract = (a: LineCounts, b: LineCounts): LineCounts => ({ added: a.added - b.added, removed: a.removed - b.removed });
/** Changed lines on screen under a collapsed set, counted from the leaves. */
function shownLines(file: DiffFile & { diff: { type: "text" } }, collapsed: ReadonlySet<number>): LineCounts {
  const { leaves, folds } = flatten(file.diff);
  const novel = novelLeaves(leaves);
  const counts = [0, 0];
  for (const side of [0, 1] as const) {
    const hidden = hiddenLines(folds[side], collapsed);
    for (const leaf of leaves[side]) {
      if (!novel.has(leaf) || collapsed.has(leaf.foldStateId)) continue;
      for (let line = leaf.startLine; line < leaf.endLine; line++) if (!hidden.has(line)) counts[side]++;
    }
  }
  return { removed: counts[0], added: counts[1] };
}
/**
 * The headline: diffr's `stats.visible` (its default fold state) adjusted by what the user
 * has folded or unfolded since. A closed file shows none.
 */
export function visibleCounts(file: DiffFile, collapsed: ReadonlySet<number>, closed = false): LineCounts {
  if (closed || file.diff.type !== "text") return zero;
  const text = file as DiffFile & { diff: { type: "text" } };
  const delta = subtract(shownLines(text, collapsed), shownLines(text, defaultCollapsed(file.diff)));
  return add(file.diff.stats.visible, delta);
}
/** GitHub's five-block bar: whole blocks by share, any non-zero side keeps one, the rest grey. */
export function blockBar(counts: LineCounts, blocks = 5): ("added" | "removed" | "neutral")[] {
  const total = counts.added + counts.removed;
  if (total === 0) return Array.from({ length: blocks }, () => "neutral");
  let green = Math.floor((blocks * counts.added) / total);
  let red = Math.floor((blocks * counts.removed) / total);
  if (counts.added > 0 && green === 0) green = 1;
  if (counts.removed > 0 && red === 0) red = 1;
  while (green + red > blocks) {
    if (green >= red) green--;
    else red--;
  }
  return [
    ...Array.from({ length: green }, () => "added" as const),
    ...Array.from({ length: red }, () => "removed" as const),
    ...Array.from({ length: blocks - green - red }, () => "neutral" as const),
  ];
}
export type Snapshot =
  | { type: "revision"; rev: string }
  | { type: "index" }
  | { type: "working_tree" }
  | { type: "empty_tree" }
  | { type: "path"; path: string };
export function snapshotLabel(snapshot: Snapshot): string {
  switch (snapshot.type) {
    case "revision":
      return /^[0-9a-f]{40}$/.test(snapshot.rev) ? snapshot.rev.slice(0, 7) : snapshot.rev;
    case "index":
      return "index";
    case "working_tree":
      return "working tree";
    case "empty_tree":
      return "empty tree";
    case "path":
      return snapshot.path;
  }
}
export const comparisonLabel = (lhs: Snapshot, rhs: Snapshot) => `${snapshotLabel(lhs)}…${snapshotLabel(rhs)}`;
