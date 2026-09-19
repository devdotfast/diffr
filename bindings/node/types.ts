// Method-free serialized diffr search contract. Field names match the Rust wire format.
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
export interface SourceData {
  text: string;
  syntax?: (Span & { capture: string })[];
  regions: RegionData[];
}
export type RegionData = {
  id: number;
  fold_state_id: number;
  start: { line: number; column: number };
  end: { line: number; column: number };
  tags?: string[];
  visibility?: Visibility;
} & (
  | { kind: "leaf"; alignment_id: number; changed?: Span[]; search_highlights?: Span[] }
  | { kind: "fold"; children: RegionData[] }
);

export interface SearchResultData {
  kind: "combined" | "lhs" | "rhs" | "unchanged";
  scope: Scope;
  file: Pairing<FileRef>;
  sources: Pairing<SourceData>;
}
