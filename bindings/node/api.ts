import { createRequire } from "node:module";
import { inspect } from "node:util";
import { bind } from "./result.ts";

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

export async function hydrate(scope: Scope, hits: Hit[]): Promise<SearchResult[]> {
  return (await native.hydrate(scope, hits)).map(bind);
}
export async function postprocess(scope: Scope, selected: SearchResult[], options?: PostprocessOptions): Promise<SearchResult[]> {
  return (await native.postprocess(scope, selected.map(result => result.toJSON()), options)).map(bind);
}
