import { createRequire } from "node:module";
import { inspect } from "node:util";

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

// The native exports currently reject with explicit not-implemented errors.
// Result construction/printing will be implemented with search, not fabricated
// here to make the snapshot pass.
interface NativeBinding {
  hydrate(scope: Scope, hits: Hit[]): Promise<SearchResult[]>;
  postprocess(scope: Scope, selected: SearchResult[]): Promise<SearchResult[]>;
}
const native: NativeBinding = createRequire(import.meta.url)("./diffr.node");
export const hydrate = native.hydrate;
export const postprocess = native.postprocess;
