/**
 * One file of the change, drawn in @pierre/diffs' DOM and stylesheet (Apache-2.0) so it looks like
 * Pierre's CodeView, but filled from diffr: structural pairing, diffr's changed spans, tree-sitter
 * colour, and folds that open and close on their syntax. Rows are windowed in chunks, so a file
 * with tens of thousands of lines only ever holds the rows near the viewport.
 */
import diffsCss from "pierre-diffs-style";
import { SVGSpriteSheet } from "pierre-diffs-sprite";
import { defaultCollapsed } from "../../../tui/packages/hunk/src/diffr/regions";
import type { FileEvent } from "../../../tui/packages/hunk/src/diffr/wire";
import type { ChangedFile } from "../github";
import type { Diffed } from "../worker";
import { buildRows, prepare, type Cell, type Layout, type Prepared, type Row } from "../model";
import { patchSize } from "../patch";
import { lineAt, lineCount } from "../lines";
import { lineHtml, tokenCss } from "../tokens";
import { icons } from "../icons";

export const LINE = 20;
export const HEADER = 44;
const SEPARATOR = 32;
const GAP = 8;
const CHUNK = 64;
/** A one-line note in place of the rows: loading, failed, hidden. */
const MESSAGE = 44;

const extraCss = `
:host {
  display: block;
  --diffs-font-family: "Geist Mono", ui-monospace, "SF Mono", Menlo, monospace;
  --diffs-header-font-family: "Geist", ui-sans-serif, system-ui, sans-serif;
  --diffs-tab-size: 4;
  --diffs-dark: #d4d4d4; --diffs-dark-bg: #171717;
  --diffs-dark-addition-color: #60d199; --diffs-dark-deletion-color: #ff6762; --diffs-dark-modified-color: #69b1ff;
  --diffs-light: #404040; --diffs-light-bg: #ffffff;
  --diffs-light-addition-color: #07c480; --diffs-light-deletion-color: #ff2e3f; --diffs-light-modified-color: #009fff;
  --diffr-warning: light-dark(#d5901c, #ffbc56);
  contain: layout paint style;
}
:host([data-theme="dark"]) { color-scheme: dark; }
:host([data-theme="light"]) { color-scheme: light; }
[data-diffs-header] { cursor: default; }
[data-header-prefix] {
  display: grid; place-items: center; width: 16px; height: 16px; padding: 0; border: 0; background: none;
  color: var(--diffs-fg-number); cursor: pointer; border-radius: 4px;
}
[data-header-prefix]:hover { color: var(--diffs-fg); }
[data-header-prefix] svg { width: 12px; height: 12px; transition: transform .12s ease; fill: currentColor; }
:host([data-collapsed]) [data-header-prefix] svg { transform: rotate(-90deg); }
[data-title] { color: var(--diffs-fg); }
[data-prev-name] { color: var(--diffs-fg-number); }
[data-rename-arrow] { color: var(--diffs-fg-number); flex: none; }
[data-tag] {
  font-size: 11px; line-height: 16px; padding: 0 6px; border-radius: 999px; flex: none;
  color: var(--diffs-fg-number); box-shadow: inset 0 0 0 1px color-mix(in lab, var(--diffs-fg-number) 35%, transparent);
}
[data-tag="fallback"] {
  display: inline-flex; align-items: center; gap: 4px; cursor: help;
  color: var(--diffr-warning); background: color-mix(in lab, var(--diffr-warning) 12%, transparent);
  box-shadow: inset 0 0 0 1px color-mix(in lab, var(--diffr-warning) 40%, transparent);
}
[data-tag="fallback"] svg { width: 12px; height: 12px; flex: none; }
[data-spacer] { grid-column: auto; }
[data-gutter] [data-spacer], [data-content] [data-spacer] { background: var(--diffs-bg); }
[data-separator-content][data-fold], [data-expand-button][data-fold] { cursor: pointer; }
[data-separator-content][data-fold]:hover { color: var(--diffs-fg); }
[data-separator][data-tint="inserted"] [data-separator-content],
[data-separator][data-tint="inserted"] [data-expand-button] { color: var(--diffs-addition-base); }
[data-separator][data-tint="removed"] [data-separator-content],
[data-separator][data-tint="removed"] [data-expand-button] { color: var(--diffs-deletion-base); }
[data-unmodified-lines] [data-count] { opacity: .6; margin-left: 1ch; }
[data-expand-button] { justify-content: center; color: var(--diffs-fg-number); }
[data-expand-button]:hover { color: var(--diffs-fg); }
[data-expand-button] svg { width: 16px; height: 16px; fill: currentColor; }
[data-fold-label] { color: var(--diffs-fg-number); font-style: italic; }
[data-fold-toggle] {
  position: absolute; left: 2px; top: 0; height: 20px; width: 14px; padding: 0; border: 0; background: none;
  display: grid; place-items: center; color: var(--diffs-fg-number); opacity: 0; cursor: pointer;
}
[data-fold-toggle] svg { width: 10px; height: 10px; fill: currentColor; }
[data-column-number]:hover [data-fold-toggle], [data-fold-toggle]:focus-visible { opacity: 1; }
[data-fold-toggle]:hover { color: var(--diffs-fg); }
[data-message] {
  display: flex; align-items: center; gap: 8px; height: 44px; box-sizing: border-box; padding: 0 16px; color: var(--diffs-fg-number);
  font-family: var(--diffs-header-font-family, var(--diffs-header-font-fallback)); background: var(--diffs-bg);
}
[data-message] button {
  font: inherit; color: var(--diffs-fg); background: var(--diffs-bg-separator); border: 0; border-radius: 6px;
  padding: 2px 10px; cursor: pointer;
}
[data-message][data-error] { color: var(--diffs-deletion-base); }
[data-filler] { background: var(--diffs-bg); }
@media (prefers-reduced-motion: reduce) { [data-shimmer]::after { animation: none; } }
[data-shimmer] { position: relative; overflow: hidden; }
[data-shimmer]::after {
  content: ""; position: absolute; inset: 0;
  background: linear-gradient(90deg, transparent, color-mix(in lab, var(--diffs-fg) 6%, transparent), transparent);
  animation: shimmer 1.2s infinite;
}
@keyframes shimmer { from { transform: translateX(-100%); } to { transform: translateX(100%); } }
${tokenCss}
`;

let sheets: CSSStyleSheet[] | undefined;
function styleSheets() {
  if (!sheets) {
    const base = new CSSStyleSheet();
    base.replaceSync(diffsCss);
    const extra = new CSSStyleSheet();
    extra.replaceSync(`@layer theme { ${extraCss} }`);
    sheets = [base, extra];
  }
  return sheets;
}

let gap: number | undefined;
/**
 * The space a code block adds below its last row: some padding plus the horizontal scrollbar's
 * gutter, which depends on the browser and its scrollbar settings, so it is measured once.
 */
function codeGap() {
  if (gap !== undefined) return gap;
  const probe = document.createElement("diffr-file");
  probe.style.cssText = "position:absolute;visibility:hidden;left:-9999px;top:0;width:600px";
  const root = probe.attachShadow({ mode: "open" });
  root.adoptedStyleSheets = styleSheets();
  root.innerHTML = `<div data-diffs-header="default"></div><pre data-diff data-indicators="bars" data-background data-diff-type="single" data-overflow="scroll">`
    + `<code data-code data-unified data-container-size><div data-gutter style="grid-row: span 1"><div data-line-type="context" data-column-number="1"><span data-line-number-content>1</span></div></div>`
    + `<div data-content style="grid-row: span 1"><div data-line="1" data-line-type="context">x</div></div></code></pre>`;
  document.body.append(probe);
  const measured = root.querySelector("pre")!.getBoundingClientRect().height - LINE;
  probe.remove();
  gap = measured >= 0 && measured < 40 ? measured : GAP;
  return gap;
}

const wanted = ["chevron", "expand-all", "symbol-added", "symbol-deleted", "symbol-modified", "symbol-moved"];
const sprite = `<svg data-icon-sprite aria-hidden="true" width="0" height="0">${wanted
  .map((id) => SVGSpriteSheet.match(new RegExp(`<symbol id="diffs-icon-${id}"[\\s\\S]*?</symbol>`))?.[0] ?? "")
  .join("")}</svg>`;
const icon = (id: string, attrs = "") => `<svg ${attrs} viewBox="0 0 16 16" width="16" height="16"><use href="#diffs-icon-${id}"></use></svg>`;

const escapes: Record<string, string> = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" };
export const escape = (text: string) => text.replace(/[&<>"]/g, (c) => escapes[c]!);

/** The header's warning when diffr fell back to a line diff, by the reason's code. */
const fallbackLabel: Record<string, string> = {
  too_complex: "Line diff · too complex",
  parse_error: "Line diff · parse errors",
  too_large: "Line diff · too large",
  generated: "Line diff · generated",
};

/** The warning's tooltip: what happened, without the CLI's advice to edit a config the page has none of. */
function fallbackTitle(problem: { message: string }) {
  return `diffr compared this file line by line instead of by its syntax: ${problem.message.replace(/; raise it in diffr config$/, "")}.`;
}

const changeType: Record<ChangedFile["status"], string> = {
  added: "new", deleted: "deleted", modified: "change", renamed: "rename-changed", copied: "rename-changed",
};
const changeIcon: Record<string, string> = {
  new: "symbol-added", deleted: "symbol-deleted", change: "symbol-modified",
  "rename-changed": "symbol-moved", "rename-pure": "symbol-moved",
};

/** Waiting: diffr has not been asked yet. Loading: diffr is on it. */
export type FileState = "waiting" | "loading" | "done" | "failed";

export interface FileHost {
  layout: Layout;
  theme: "dark" | "light";
  changed(view: FileView): void;
}

/** Height of each row and where it starts, for windowing. */
interface Layouted {
  rows: Row[];
  heights: Float64Array;
  offsets: Float64Array;
  body: number;
}

export class FileView {
  readonly element: HTMLElement;
  private root: ShadowRoot;
  state: FileState = "waiting";
  event?: FileEvent;
  error?: string;
  prepared?: Prepared;
  collapsed = new Set<number>();
  /** The reader collapsed the whole file. */
  closed = false;
  /** A file diffr hides by default stays behind "Load diff" until asked. */
  revealed = false;
  private layout?: Layouted;
  /** Bumped whenever folds, layout, or the result change; the rows and the height key on it. */
  private version = 0;
  private layoutKey = -1;
  private bodyHeight?: { key: number; height: number };
  private window: [number, number] = [-1, -1];
  private renderedKey = "";
  /** The placeholder's height for a layout, measured from GitHub's patch. */
  private estimate?: { layout: Layout; height: number };
  /** Milliseconds diffr spent on this file, for the engine panel. */
  diffMs = 0;

  constructor(readonly file: ChangedFile, readonly index: number, private host: FileHost) {
    this.element = document.createElement("diffr-file");
    this.element.dataset.index = String(index);
    this.root = this.element.attachShadow({ mode: "open" });
    this.root.adoptedStyleSheets = styleSheets();
    this.root.addEventListener("click", (event) => this.onClick(event as MouseEvent));
    this.element.style.height = `${this.height()}px`;
  }

  get path() {
    return this.file.path;
  }

  /** diffr has not answered yet: the file holds room for its diff. */
  private get pending() {
    return !this.prepared && (this.state === "waiting" || this.state === "loading");
  }

  /** Room for the diff before diffr has it: GitHub's patch's rows, or its changed lines without one. */
  private placeholder(): number {
    const patch = this.file.patch;
    if (!patch) return Math.max(MESSAGE, (this.file.additions + this.file.deletions) * LINE + GAP);
    // Every file's height is read each time any diff lands, so the patch is measured once a layout.
    const layout = this.layoutMode;
    if (this.estimate?.layout !== layout) {
      const { lines, gaps } = patchSize(patch, layout === "split");
      this.estimate = { layout, height: Math.max(MESSAGE, lines * LINE + gaps * (SEPARATOR + 2 * GAP) + codeGap()) };
    }
    return this.estimate.height;
  }

  markLoading() {
    this.state = "loading";
    this.renderedKey = "";
  }

  setResult(result: Diffed | undefined, error?: string) {
    const event = result?.event;
    this.event = event;
    this.error = error;
    this.state = error || !event ? "failed" : "done";
    const diff = event?.diff;
    if (diff?.type === "text") {
      this.prepared = prepare(diff, result!.lines, result!.syntax);
      this.collapsed = defaultCollapsed(diff);
    }
    this.invalidate();
  }

  /** Counts for the header: diffr's under its default fold state, or GitHub's before that. */
  get stats() {
    const diff = this.event?.diff;
    if (diff?.type === "text") return diff.stats.visible;
    return this.state === "done" || this.state === "failed" ? undefined : { added: this.file.additions, removed: this.file.deletions };
  }

  /** An added or deleted file has one side, so it reads as one column in either layout. */
  private get layoutMode(): Layout {
    return this.file.status === "added" || this.file.status === "deleted" ? "unified" : this.host.layout;
  }

  private hiddenByDefault() {
    return !!this.event?.visibility.collapsed && !this.revealed;
  }

  invalidate() {
    this.version++;
    this.renderedKey = "";
    this.window = [-1, -1];
    this.element.style.height = `${this.height()}px`;
  }

  private ensureLayout(): Layouted | undefined {
    if (this.closed) return undefined;
    if (!this.prepared || this.hiddenByDefault()) return undefined;
    const key = this.version;
    if (this.layout && key === this.layoutKey) return this.layout;
    const rows = buildRows(this.prepared, this.layoutMode, this.collapsed);
    const heights = new Float64Array(rows.length);
    rows.forEach((row, i) => {
      if (row.type !== "fold") heights[i] = LINE;
      else {
        // A separator sits in a band of the gap above and below it, except at the file's edges.
        const top = i === 0 ? 0 : GAP, bottom = i === rows.length - 1 ? 0 : GAP;
        heights[i] = SEPARATOR + top + bottom + row.extra.length * LINE;
      }
    });
    const offsets = new Float64Array(rows.length + 1);
    for (let i = 0; i < rows.length; i++) offsets[i + 1] = offsets[i]! + heights[i]!;
    // Under a header the code block has no top padding; at the bottom it keeps the gap, as padding
    // plus the horizontal scrollbar's gutter.
    this.layout = { rows, heights, offsets, body: offsets[rows.length]! + codeGap() };
    this.layoutKey = key;
    this.bodyHeight = { key, height: rows.length ? this.layout.body : MESSAGE };
    return this.layout;
  }

  /** The file's full height, header included, whether or not its rows are on screen. */
  height(): number {
    if (this.closed) return HEADER;
    if (this.pending) return HEADER + this.placeholder();
    if (this.state !== "done" || !this.prepared || this.hiddenByDefault()) return HEADER + MESSAGE;
    // Off screen a file keeps only its height; its rows are built again when it comes back.
    if (this.bodyHeight?.key !== this.version) this.ensureLayout();
    return HEADER + this.bodyHeight!.height;
  }

  /** Draw the rows between `top` and `bottom`, in this file's own coordinates. */
  render(top: number, bottom: number) {
    const layout = this.ensureLayout();
    let window: [number, number] = [0, 0];
    if (layout && layout.rows.length) {
      const first = Math.max(0, upperBound(layout.offsets, top - HEADER) - 1);
      const last = Math.min(layout.rows.length, upperBound(layout.offsets, bottom - HEADER));
      window = [Math.floor(first / CHUNK) * CHUNK, Math.min(layout.rows.length, Math.ceil(last / CHUNK) * CHUNK)];
    }
    const key = `${this.host.layout}|${this.host.theme}|${this.version}|${this.state}|${this.closed}|${this.revealed}`;
    if (key === this.renderedKey && window[0] === this.window[0] && window[1] === this.window[1]) return;
    this.renderedKey = key;
    this.window = window;
    this.element.dataset.theme = this.host.theme;
    this.element.toggleAttribute("data-collapsed", this.closed);
    this.root.innerHTML = sprite + this.headerHtml() + (this.closed ? "" : this.bodyHtml(layout, window));
  }

  /** Drop everything but the placeholder height while the file is far off screen. */
  release() {
    this.layout = undefined;
    if (this.renderedKey === "") return;
    this.root.innerHTML = "";
    this.renderedKey = "";
    this.window = [-1, -1];
  }

  private headerHtml() {
    const type = changeType[this.file.status];
    const prev = this.file.previousPath && this.file.previousPath !== this.file.path
      ? `<div data-prev-name><bdi>${escape(this.file.previousPath)}</bdi></div><span data-rename-arrow>→</span>` : "";
    const tags: string[] = [];
    const diff = this.event?.diff;
    const empty = !!this.prepared && !this.prepared.texts[0].text && !this.prepared.texts[1].text;
    if (diff?.type === "text" && diff.stats.fallback && !empty)
      tags.push(`<span data-tag="fallback" title="${escape(fallbackTitle(diff.stats.fallback))}">${icons.warning}${fallbackLabel[diff.stats.fallback.code] ?? "Line diff"}</span>`);
    if (diff?.type === "binary") tags.push(`<span data-tag>binary</span>`);
    const stats = this.stats;
    const counts = stats
      ? `${stats.removed ? `<span data-deletions-count>-${stats.removed}</span>` : ""}${stats.added ? `<span data-additions-count>+${stats.added}</span>` : ""}`
      : "";
    return `<div data-diffs-header="default" data-change-type="${type}" data-sticky>`
      + `<div data-header-content><button data-header-prefix data-toggle-file title="${this.closed ? "Expand file" : "Collapse file"}">${icon("chevron")}</button>`
      + icon(changeIcon[type]!, `data-change-icon="${type}"`)
      + `${prev}<div data-title><bdi>${escape(this.file.path)}</bdi></div>${tags.join("")}</div>`
      + `<div data-metadata>${counts}</div></div>`;
  }

  private bodyHtml(layout: Layouted | undefined, [from, to]: [number, number]) {
    if (this.pending) {
      const filler = this.height() - HEADER - MESSAGE;
      return `<div data-message${this.state === "loading" ? " data-shimmer" : ""}>${this.state === "loading" ? "Diffing with diffr…" : "Diffed when it comes into view"}</div>`
        + (filler > 0 ? `<div data-filler style="height:${filler}px"></div>` : "");
    }
    if (this.state === "failed") return `<div data-message data-error>${escape(this.error ?? this.event?.error?.message ?? "diffr failed on this file")}</div>`;
    if (this.event?.error) return `<div data-message data-error>${escape(this.event.error.message)}</div>`;
    const diff = this.event?.diff;
    if (!diff) return `<div data-message>No diff</div>`;
    if (diff.type === "binary") return `<div data-message>Binary file not shown</div>`;
    if (this.hiddenByDefault())
      return `<div data-message><button data-reveal>Load diff</button>${escape(this.event!.visibility.label || "Hidden by default")}</div>`;
    if (!layout || !layout.rows.length) {
      const empty = !this.prepared?.texts[0].text && !this.prepared?.texts[1].text;
      return `<div data-message>${empty ? "Empty file" : "No changes to show"}</div>`;
    }
    const p = this.prepared!;
    return this.codeHtml(layout, [from, to], Math.max(lineCount(p.texts[0]), lineCount(p.texts[1])));
  }

  private codeHtml(layout: Layouted, [from, to]: [number, number], lines: number) {
    const digits = Math.max(3, String(lines).length);
    const before = layout.offsets[from]!, after = layout.offsets[layout.rows.length]! - layout.offsets[to]!;
    const rows = layout.rows.slice(from, to);
    const last = layout.rows.length - 1;
    const split = this.layoutMode === "split";
    const pre = `<pre data-diff data-indicators="bars" data-background data-diff-type="${split ? "split" : "single"}" data-overflow="scroll" style="--diffs-min-number-column-width-default:${digits}ch">`;
    if (split)
      return pre + this.splitCode(0, rows, from, last, before, after) + this.splitCode(1, rows, from, last, before, after) + `</pre>`;
    return pre + this.unifiedCode(rows, from, last, before, after) + `</pre>`;
  }

  private separator(row: Extract<Row, { type: "fold" }>, index: number, last: number, content: boolean) {
    const edge = `${index === 0 ? " data-separator-first" : ""}${index === last ? " data-separator-last" : ""}`;
    const count = row.label.match(/^\d/) ? "" : `<span data-count>${row.lines.toLocaleString()} lines</span>`;
    return `<div data-separator="line-info" data-expand-index="${index}" data-tint="${row.tint}"${edge}>`
      + (content ? "" : `<div data-separator-wrapper><div data-expand-button data-expand-both data-fold="${row.id}" title="Expand">${icon("expand-all")}</div>`
        + `<div data-separator-content data-fold="${row.id}"><span data-unmodified-lines>${escape(row.label)}${count}</span></div></div>`)
      + `</div>`;
  }

  private numberCell(cell: Cell, number: number) {
    const toggle = cell.fold && !cell.fold.collapsed
      ? `<button data-fold-toggle data-fold="${cell.fold.id}" title="Fold${cell.fold.label ? `: ${escape(cell.fold.label.split("\n")[0]!)}` : ""}">${icon("chevron")}</button>` : "";
    return `<div data-line-type="${cell.kind}" data-column-number="${number}">${toggle}<span data-line-number-content>${number}</span></div>`;
  }

  private lineCell(cell: Cell, number: number, alt?: number) {
    const p = this.prepared;
    const text = lineAt(p!.texts[cell.side], cell.line);
    return `<div data-line="${number}"${alt !== undefined ? ` data-alt-line="${alt}"` : ""} data-line-type="${cell.kind}">`
      + `${lineHtml(text, p!.syntax[cell.side], cell.line, cell.changed)}</div>`;
  }

  private splitCode(side: 0 | 1, rows: Row[], from: number, last: number, before: number, after: number) {
    let gutter = "", content = "", count = 0, buffer = 0;
    const flushBuffer = () => {
      if (!buffer) return;
      const style = `grid-row: span ${buffer};min-height:calc(${buffer} * 1lh);`;
      gutter += `<div data-gutter-buffer="buffer" data-buffer-size="${buffer}" style="${style}"></div>`;
      content += `<div data-content-buffer data-buffer-size="${buffer}" style="${style}"></div>`;
      count += buffer;
      buffer = 0;
    };
    const spacer = (height: number) => {
      if (height <= 0) return;
      flushBuffer();
      gutter += `<div data-spacer style="height:${height}px"></div>`;
      content += `<div data-spacer style="height:${height}px"></div>`;
      count++;
    };
    spacer(before);
    rows.forEach((row, i) => {
      if (row.type === "fold") {
        flushBuffer();
        gutter += this.separator(row, from + i, last, false);
        content += this.separator(row, from + i, last, true);
        count++;
        for (const text of row.extra) {
          gutter += `<div data-line-type="context" data-column-number=""></div>`;
          content += `<div data-line data-line-type="context" data-fold-label>${escape(text)}</div>`;
          count++;
        }
        return;
      }
      if (!("left" in row) && !("right" in row)) return;
      const cell = side ? (row as { right?: Cell }).right : (row as { left?: Cell }).left;
      if (!cell) {
        buffer++;
        return;
      }
      flushBuffer();
      gutter += this.numberCell(cell, cell.line + 1);
      content += this.lineCell(cell, cell.line + 1);
      count++;
    });
    flushBuffer();
    spacer(after);
    const attr = side ? "data-additions" : "data-deletions";
    return `<code data-code ${attr} data-container-size><div data-gutter style="grid-row: span ${count}">${gutter}</div>`
      + `<div data-content style="grid-row: span ${count}">${content}</div></code>`;
  }

  private unifiedCode(rows: Row[], from: number, last: number, before: number, after: number) {
    let gutter = "", content = "", count = 0;
    const spacer = (height: number) => {
      if (height <= 0) return;
      gutter += `<div data-spacer style="height:${height}px"></div>`;
      content += `<div data-spacer style="height:${height}px"></div>`;
      count++;
    };
    spacer(before);
    rows.forEach((row, i) => {
      if (row.type === "fold") {
        gutter += this.separator(row, from + i, last, false);
        content += this.separator(row, from + i, last, true);
        count++;
        for (const text of row.extra) {
          gutter += `<div data-line-type="context" data-column-number=""></div>`;
          content += `<div data-line data-line-type="context" data-fold-label>${escape(text)}</div>`;
          count++;
        }
        return;
      }
      const cell = (row as { cell: Cell }).cell;
      gutter += this.numberCell(cell, cell.line + 1);
      content += this.lineCell(cell, cell.line + 1, cell.alt !== undefined ? cell.alt + 1 : undefined);
      count++;
    });
    spacer(after);
    return `<code data-code data-unified data-container-size><div data-gutter style="grid-row: span ${count}">${gutter}</div>`
      + `<div data-content style="grid-row: span ${count}">${content}</div></code>`;
  }

  private onClick(event: MouseEvent) {
    const target = event.target as Element;
    const fold = target.closest<HTMLElement>("[data-fold]");
    if (fold) {
      const id = Number(fold.dataset.fold);
      if (this.collapsed.has(id)) this.collapsed.delete(id);
      else this.collapsed.add(id);
    } else if (target.closest("[data-toggle-file]")) this.closed = !this.closed;
    else if (target.closest("[data-reveal]")) this.revealed = true;
    else return;
    this.invalidate();
    this.host.changed(this);
  }

  expandAll() {
    this.collapsed.clear();
    this.invalidate();
  }

  resetFolds() {
    if (this.event?.diff?.type === "text") this.collapsed = defaultCollapsed(this.event.diff);
    this.invalidate();
  }
}

/** First index whose value exceeds `value`. */
function upperBound(values: Float64Array, value: number) {
  let lo = 0, hi = values.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (values[mid]! <= value) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}
