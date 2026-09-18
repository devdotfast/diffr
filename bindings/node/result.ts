import { inspect } from "node:util";
import { print } from "./print.ts";
import type { Region, SearchResult, SearchResultData, Scope, Pairing, FileRef, Source } from "./api.ts";

function walk(regions: Region[], visit: (region: Region) => void): void {
  for (const region of regions) {
    visit(region);
    if (region.kind === "fold") walk(region.children, visit);
  }
}
export function bind(data: SearchResultData): SearchResult {
  for (const source of Object.values(data.sources)) {
    source.regions ??= [];
    walk(source.regions, region => {
      const anyLeaf = (predicate: (leaf: Extract<Region, { kind: "leaf" }>) => boolean) => {
        let found = false;
        walk([region], child => { if (child.kind === "leaf") found ||= predicate(child); });
        return found;
      };
      Object.defineProperties(region, {
        hasChanges: { value: () => anyLeaf(leaf => Boolean(leaf.changed?.length)) },
        hasHighlights: { value: () => anyLeaf(leaf => Boolean(leaf.search_highlights?.length)) },
        hasChangedHighlights: { value: () => anyLeaf(leaf => (leaf.search_highlights ?? []).some(hit =>
          (leaf.changed ?? []).some(change => change.line === hit.line && change.start_column < hit.end_column && hit.start_column < change.end_column))) },
      });
    });
  }
  return Object.assign(Object.create(Result.prototype), data);
}
class Result implements SearchResult {
  declare kind: SearchResultData["kind"];
  declare scope: Scope;
  declare file: Pairing<FileRef>;
  declare sources: Pairing<Source>;
  setCollapsed(foldStateId: number, collapsed: boolean): void {
    if (!Number.isInteger(foldStateId) || foldStateId < 0 || typeof collapsed !== "boolean") throw new TypeError("expected a fold-state ID and boolean");
    let found = false;
    for (const source of Object.values(this.sources)) walk(source.regions, region => {
      if (region.fold_state_id === foldStateId) {
        region.visibility = { ...region.visibility, collapsed };
        found = true;
      }
    });
    if (!found) throw new RangeError(`No fold_state_id ${foldStateId} in this result`);
  }
  toString(): string { return print(this); }
  [inspect.custom](): string { return this.toString(); }
  toJSON(): SearchResultData { return { kind: this.kind, scope: this.scope, file: this.file, sources: this.sources }; }
}
