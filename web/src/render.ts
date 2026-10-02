/** Paint the TUI's rows as HTML: the same zip, folds and colours, in a browser. */
import type { FoldTint, RowFold } from "../../tui/packages/hunk/src/diffr/regions";
import type { Layout, Palette, ViewerRow } from "../../tui/packages/hunk/src/diffr/rows";
import type { RenderSpan, SplitLineCell, UnifiedLineCell } from "../../tui/packages/hunk/src/ui/diff/diffRowModel";

export const escape = (text: string) =>
  text.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);

/** The palette as CSS custom properties, so the stylesheet follows the TUI's theme. */
export function paletteVars(theme: Palette): string {
  const vars: Record<string, string> = {
    bg: theme.bg, fg: theme.fg, muted: theme.muted, chrome: theme.chrome, highlight: theme.highlight,
    addition: theme.addition, deletion: theme.deletion, "add-word": theme.addWord, "delete-word": theme.deleteWord,
    "added-text": theme.addedText, "removed-text": theme.removedText, accent: theme.accent,
    "fold-bg": theme.foldBackground, "fold-fg": theme.foldPlaceholder,
  };
  return Object.entries(vars).map(([name, value]) => `--${name}:${value}`).join(";");
}

const tintClass = (tint: FoldTint | undefined) => (tint ? ` tint-${tint}` : "");

function spans(list: RenderSpan[]): string {
  return list
    .map((span) => {
      const style = [span.fg && `color:${span.fg}`, span.bg && `background:${span.bg}`].filter(Boolean).join(";");
      return style ? `<span style="${style}">${escape(span.text)}</span>` : escape(span.text);
    })
    .join("");
}

/** The chevron on a line that starts a region, or the whole row of a collapsed one. */
function chevron(fold: RowFold | undefined): string {
  if (!fold) return `<span class="chev"></span>`;
  const title = fold.collapsed ? "Expand" : "Collapse";
  return `<button class="chev" data-fold="${fold.id}" title="${title}" aria-expanded="${!fold.collapsed}">${fold.collapsed ? "▸" : "▾"}</button>`;
}

function code(cell: SplitLineCell | UnifiedLineCell): string {
  const fold = cell.fold;
  if (fold?.collapsed) {
    // A multi-line label hangs under the row; the row itself is a bare ellipsis.
    const label = fold.label && !fold.label.includes("\n") ? fold.label : "⋯";
    return `<button class="code folded" data-fold="${fold.id}">${escape(label)}</button>`;
  }
  return `<span class="code">${spans(cell.spans)}</span>`;
}

function cellClass(cell: SplitLineCell | UnifiedLineCell): string {
  if (cell.fold?.collapsed) return `cell collapsed${tintClass(cell.fold.tint)}`;
  if (cell.foldLabel) return `cell fold-label${tintClass(cell.foldTint)}`;
  return `cell ${cell.kind}`;
}

function splitCell(cell: SplitLineCell | undefined): string {
  if (!cell || cell.kind === "empty") return `<div class="cell empty"></div>`;
  return `<div class="${cellClass(cell)}"><span class="ln">${cell.lineNumber ?? ""}</span>${chevron(cell.fold)}`
    + `<span class="sign">${cell.foldLabel ? "" : escape(cell.sign.trim())}</span>${code(cell)}</div>`;
}

function unifiedCell(cell: UnifiedLineCell): string {
  return `<div class="${cellClass(cell)}"><span class="ln">${cell.oldLineNumber ?? ""}</span>`
    + `<span class="ln">${cell.newLineNumber ?? ""}</span>${chevron(cell.fold)}`
    + `<span class="sign">${cell.foldLabel ? "" : escape(cell.sign.trim())}</span>${code(cell)}</div>`;
}

export function renderRows(rows: ViewerRow[], layout: Layout): string {
  let html = "";
  for (const row of rows) {
    // rowsForFile leads with a header row; the page draws its own.
    if (!row.left && !row.right && !row.cell) continue;
    const hunk = row.hunkStart ? " hunk" : "";
    html += layout === "split"
      ? `<div class="row split${hunk}">${splitCell(row.left)}${splitCell(row.right)}</div>`
      : `<div class="row unified${hunk}">${unifiedCell(row.cell!)}</div>`;
  }
  return html;
}
