import { createRequire } from "node:module";
import { inspect } from "node:util";
import { print } from "./print.ts";

export interface Scope {
  repo: string;
  baseWorktree: { commitId: string; path: string };
  headWorktree: { commitId: string; path: string };
}

export interface Hit {
  file: string;
  lines: { line: number; text: string }[];
}

export type Pairing<T> = { lhs: T; rhs: T } | { lhs: T; rhs?: never } | { lhs?: never; rhs: T };
export interface Span { line: number; start_column: number; end_column: number }
export interface Visibility { collapsed?: boolean; label?: string }
export interface FileRef { path: string; oid: string; mode: string }
export interface Source {
  text: string;
  syntax?: (Span & { capture: string })[];
  regions: Region[];
}
export type Region = {
  id: number;
  fold_state_id: number;
  start: { line: number; column: number };
  end: { line: number; column: number };
  tags?: string[];
  visibility?: Visibility;
  hasChanges(): boolean;
  hasHighlights(): boolean;
  hasChangedHighlights(): boolean;
} & (
  | { kind: "leaf"; alignment_id: number; changed?: Span[]; search_highlights?: Span[] }
  | { kind: "fold"; children: Region[] }
);

export interface SearchResultData {
  kind: "combined" | "lhs" | "rhs" | "unchanged";
  scope: Scope;
  file: Pairing<FileRef>;
  sources: Pairing<Source>;
}
export interface SearchResult extends SearchResultData {
  setCollapsed(foldStateId: number, collapsed: boolean): void;
  toString(): string;
  [inspect.custom](): string;
  toJSON(): SearchResultData;
}

export interface PluginConfig {
  order?: string[];
  bundled?: Record<string, { enabled?: boolean; [option: string]: unknown }>;
  external?: Record<string, { path: string; enabled?: boolean; [option: string]: unknown }>;
}
export interface PostprocessOptions { plugins?: PluginConfig }

interface NativeBinding {
  hydrate(scope: Scope, hits: Hit[]): Promise<SearchResultData[]>;
  postprocess(scope: Scope, selected: SearchResultData[], options?: PostprocessOptions): Promise<SearchResultData[]>;
}
const native: NativeBinding = createRequire(import.meta.url)("./diffr.node");

function walk(regions: Region[], visit: (region: Region) => void): void {
  for (const region of regions) {
    visit(region);
    if (region.kind === "fold") walk(region.children, visit);
  }
}
function bind(data: SearchResultData): SearchResult {
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
export async function hydrate(scope: Scope, hits: Hit[]): Promise<SearchResult[]> {
  return (await native.hydrate(scope, hits)).map(bind);
}
export async function postprocess(scope: Scope, selected: SearchResult[], options?: PostprocessOptions): Promise<SearchResult[]> {
  return (await native.postprocess(scope, selected.map(result => result.toJSON()), options)).map(bind);
}
