/**
 * Declares the terminal diff row model: the row and span shapes every review surface
 * measures, windows, and paints.
 *
 * Kept as a leaf module so both the row builders (`diffRows.ts`) and their consumers —
 * column math, the highlight worker, geometry — can share these types without importing
 * the builders themselves.
 */
import type { RowFold } from "../../diffr/regions";
type DiffLineMoveKind = "moved";

export interface RenderSpan {
  text: string;
  fg?: string;
  bg?: string;
  /** Resolve paint-only foreground effects after cursor and copy-selection backgrounds apply. */
  transformFg?: (sourceFg: string | undefined, renderedBg: string) => string;
}

export interface SplitLineCell {
  kind: "context" | "addition" | "deletion" | "empty";
  sign: string;
  lineNumber?: number;
  moveKind?: DiffLineMoveKind;
  /** This cell starts a fold region; the chevron and placeholder come from here. */
  fold?: RowFold;
  /** A line of a collapsed fold's label, painted in the fold tint without a line number. */
  foldLabel?: boolean;
  spans: RenderSpan[];
}

export interface UnifiedLineCell {
  kind: "context" | "addition" | "deletion";
  sign: string;
  oldLineNumber?: number;
  newLineNumber?: number;
  moveKind?: DiffLineMoveKind;
  fold?: RowFold;
  foldLabel?: boolean;
  spans: RenderSpan[];
}
