import { createRequire } from "node:module";
import { inspect } from "node:util";
import { bind } from "./result.ts";

export type { Scope, Hit, Comparison, Span, Visibility, FileRef, RegionData, SourceData, SearchResultData } from "./types.ts";
import type { Scope, Hit, Comparison, FileRef, RegionData, SourceData, SearchResultData } from "./types.ts";

export type Region = (Extract<RegionData, { kind: "leaf" }> |
  (Omit<Extract<RegionData, { kind: "fold" }>, "children"> & { children: Region[] })) & {
  hasChanges(): boolean;
  hasHighlights(): boolean;
  hasChangedHighlights(): boolean;
};
export interface Source extends Omit<SourceData, "regions"> {
  regions: Region[];
}
export type SearchResult = {
  scope: Scope;
  display: "both" | "lhs" | "rhs";
  setCollapsed(foldStateId: number, collapsed: boolean): void;
  toString(): string;
  [inspect.custom](): string;
  toJSON(): SearchResultData;
} & Comparison<FileRef, Source>;

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

export async function hydrate(scope: Scope, hits: Hit[]): Promise<SearchResult[]> {
  return (await native.hydrate(scope, hits)).map(bind);
}
export async function postprocess(scope: Scope, selected: SearchResult[], options?: PostprocessOptions): Promise<SearchResult[]> {
  return (await native.postprocess(scope, selected.map(result => result.toJSON()), options)).map(bind);
}
