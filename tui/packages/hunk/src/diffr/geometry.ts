/** Measure once per layout change, then window measured rows using Hunk's binary search. */
import { sliceSpansWindow, wrapSpans } from "../ui/diff/styledSpanLayout";
import { resolveVisibleRowIndexWindow } from "../ui/diff/rowWindowing";
import type { RenderSpan } from "../ui/diff/diffRowModel";
import type { ViewerRow } from "./rows";
export interface MeasuredRow {
  row: ViewerRow;
  top: number;
  height: number;
  left: RenderSpan[][];
  right: RenderSpan[][];
  cell: RenderSpan[][];
}
export interface Geometry {
  rows: MeasuredRow[];
  height: number;
  leftWidth: number;
  rightWidth: number;
  /** Split gutter: padding, line number, gap, fold chevron, gap. */
  gutter: number;
  /** Unified gutter: padding, old number, gap, new number, gap, chevron, gap. */
  unifiedGutter: number;
}
export function measureRows(
  rows: ViewerRow[],
  width: number,
  wrap: boolean,
  horizontalOffset: number,
): Geometry {
  const maxLine = rows.reduce(
    (max, r) =>
      Math.max(
        max,
        r.left?.lineNumber ?? 0,
        r.right?.lineNumber ?? 0,
        r.cell?.oldLineNumber ?? 0,
        r.cell?.newLineNumber ?? 0,
      ),
    1,
  );
  const digits = String(maxLine).length;
  const gutter = digits + 4, unifiedGutter = digits * 2 + 5;
  const leftWidth = Math.floor((width - 1) / 2),
    rightWidth = width - leftWidth - 1;
  const measure = (spans: RenderSpan[] | undefined, available: number) => {
    if (!spans) return [];
    return wrap
      ? wrapSpans(spans, Math.max(1, available))
      : [
          sliceSpansWindow(spans, horizontalOffset, Math.max(1, available))
            .spans,
        ];
  };
  let top = 0;
  const measured = rows.map((row) => {
    const left = measure(row.left?.spans, leftWidth - gutter),
      right = measure(row.right?.spans, rightWidth - gutter);
    const cell = measure(row.cell?.spans, width - unifiedGutter);
    const height = Math.max(1, left.length, right.length, cell.length);
    const result = { row, top, height, left, right, cell };
    top += height;
    return result;
  });
  return { rows: measured, height: top, leftWidth, rightWidth, gutter, unifiedGutter };
}
export function visibleRows(geometry: Geometry, top: number, height: number) {
  const window = resolveVisibleRowIndexWindow({
    bodyHeight: geometry.height,
    rowBounds: geometry.rows,
    visibleBodyBounds: { top, height },
  });
  return geometry.rows.slice(window.startIndex, window.endIndex);
}
