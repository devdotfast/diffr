import type { Region, SearchResultData, Source } from "./api";

type Row = { key: string; line: number; text: string; changed: boolean };
type Fold = { key: string; first: number; last: number; id: number };
type Item = Row | Fold;
const isFold = (item: Item): item is Fold => "id" in item;
const number = (line?: number) => line === undefined ? "     " : String(line + 1).padStart(5);

function visible(source: Source): Item[] {
  const lines = source.text.split("\n").map(line => line.replace(/\r$/, ""));
  const items: Item[] = [];
  function visit(regions: Region[]) {
    for (const region of regions) {
      const first = region.start.line;
      const end = region.end.line + Number(region.end.column > 0);
      if (region.visibility?.collapsed) {
        if (end > first) items.push({ key: `fold:${region.fold_state_id}`, id: region.fold_state_id, first, last: end - 1 });
      } else if (region.kind === "fold") {
        visit(region.children);
      } else {
        for (let line = first; line < end; line++) {
          items.push({ key: `row:${region.alignment_id}:${line - first}`, line, text: lines[line],
            changed: (region.changed ?? []).some(span => span.line === line) });
        }
      }
    }
  }
  visit(source.regions);
  return items;
}

/** Merge ordered visibility streams by their shared alignment/fold identities. */
function pair(left: Item[], right: Item[]): [Item | undefined, Item | undefined][] {
  const rows: [Item | undefined, Item | undefined][] = [];
  let l = 0, r = 0;
  while (l < left.length) {
    const matching = right.findIndex((item, index) => index >= r && item.key === left[l].key);
    if (matching < 0) { rows.push([left[l++], undefined]); continue; }
    while (r < matching) rows.push([undefined, right[r++]]);
    rows.push([left[l++], right[r++]]);
  }
  while (r < right.length) rows.push([undefined, right[r++]]);
  return rows;
}

export function print(result: SearchResultData): string {
  const { lhs, rhs } = result.sources;
  const leftPath = result.file.lhs?.path;
  const rightPath = result.file.rhs?.path;
  const name = leftPath && rightPath && leftPath !== rightPath ? `${leftPath} → ${rightPath}` : rightPath ?? leftPath;
  const unchanged = result.kind === "unchanged";
  const combined = Boolean(lhs && rhs) && !unchanged;
  const output = [`${name} — ${unchanged ? (lhs && rhs ? "base = head" : lhs ? "base" : "head") : combined ? "base → head" : lhs ? "base" : "head"}`,
    combined ? " base  head" : unchanged ? " line" : lhs ? " base" : " head"];
  let folded = false;
  const range = (fold: Fold, side: string) => `${side} ${fold.first + 1}–${fold.last + 1}`;
  const foldRow = (left?: Fold, right?: Fold) => {
    folded = true;
    const ranges = [left && range(left, "base"), right && range(right, "head")].filter(Boolean).join(" / ");
    output.push(`              … ${ranges} collapsed [fold_state_id=${(left ?? right)!.id}] …`);
  };
  if (combined) {
    for (const [left, right] of pair(visible(lhs!), visible(rhs!))) {
      if ((left && isFold(left)) || (right && isFold(right))) {
        foldRow(left && isFold(left) ? left : undefined, right && isFold(right) ? right : undefined);
        continue;
      }
      const l = left as Row | undefined, r = right as Row | undefined;
      if (l && r && !l.changed && !r.changed && l.text === r.text) {
        output.push(`${number(l.line)} ${number(r.line)}   ${r.text}`.trimEnd());
      } else {
        if (l) output.push(`${number(l.line)}       - ${l.text}`.trimEnd());
        if (r) output.push(`      ${number(r.line)} + ${r.text}`.trimEnd());
      }
    }
  } else {
    // Identical sources can have different selected hits; merge visibility before
    // rendering so evidence from either side remains visible in a single excerpt.
    const items = lhs && rhs ? pair(visible(lhs), visible(rhs)).map(([l,r]) => r ?? l!) : visible((lhs ?? rhs)!);
    for (const item of items) {
      if (isFold(item)) {
        foldRow(lhs ? item : undefined, rhs ? item : undefined);
      } else {
        output.push(`${number(item.line)} ${unchanged ? " " : lhs ? "-" : "+"} ${item.text}`.trimEnd());
      }
    }
  }
  if (folded) output.push("", "[More context: call result.setCollapsed(fold_state_id, false) with an indicated\nID, then print the result again. Full source text and region children are already\npresent; no read call is needed.]");
  return output.join("\n");
}
