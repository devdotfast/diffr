/**
 * The column of files. Every file keeps its full height in the flow, so the scrollbar is true from
 * the start, but only the files near the viewport hold any rows; the rest are empty boxes.
 */
import type { FileView } from "./file";

/** Pixels beyond the viewport that are drawn ahead of a scroll. */
const AHEAD = 1200;
/** Files further than this from the viewport give their rows back. */
const KEEP = 2400;
/** The line between two files. */
const GAP = 1;

export class Viewer {
  readonly element: HTMLElement;
  private list: HTMLElement;
  private views: FileView[] = [];
  private visible: FileView[] = [];
  private tops: number[] = [];
  private frame = 0;
  /** Called with the file at the top of the viewport whenever it changes. */
  onCurrent?: (view: FileView) => void;
  private current?: FileView;
  /** Where a jump to a file left the scroll: until the reader moves, that file stays current. */
  private jumped?: number;

  constructor(element: HTMLElement) {
    this.element = element;
    this.list = document.createElement("div");
    this.list.className = "files";
    element.append(this.list);
    element.addEventListener("scroll", () => this.schedule(), { passive: true });
    new ResizeObserver(() => this.schedule()).observe(element);
  }

  setViews(views: FileView[]) {
    this.views = views;
    this.list.replaceChildren(...views.map((view) => view.element));
    this.setVisible(views);
  }

  /** Show only these files (a filter); the others leave the flow. */
  setVisible(visible: FileView[]) {
    const shown = new Set(visible);
    for (const view of this.views) view.element.hidden = !shown.has(view);
    this.visible = visible;
    this.measure();
  }

  /** Heights changed: a diff arrived, a fold toggled, the layout switched. */
  measure() {
    let top = 0;
    this.tops = this.visible.map((view) => {
      const at = top;
      top += view.height() + GAP;
      return at;
    });
    this.schedule();
  }

  schedule() {
    if (!this.frame) this.frame = requestAnimationFrame(() => this.paint());
  }

  /** Draw what is on screen now, without waiting for a frame. */
  paint() {
    if (this.frame) cancelAnimationFrame(this.frame);
    this.frame = 0;
    const top = this.element.scrollTop, height = this.element.clientHeight;
    let current: FileView | undefined;
    this.visible.forEach((view, i) => {
      const start = this.tops[i]!, end = start + view.height();
      if (end >= top - AHEAD && start <= top + height + AHEAD) view.render(top - AHEAD - start, top + height + AHEAD - start);
      else if (end < top - KEEP || start > top + height + KEEP) view.release();
      if (!current && end > top + 1) current = view;
    });
    if (this.jumped !== undefined && Math.abs(top - this.jumped) > 1) this.jumped = undefined;
    if (current && current !== this.current && this.jumped === undefined) {
      this.current = current;
      this.onCurrent?.(current);
    }
  }

  /** The file at the top of the viewport. */
  get currentView() {
    return this.current;
  }

  scrollTo(view: FileView) {
    const i = this.visible.indexOf(view);
    if (i < 0) return;
    this.element.scrollTop = this.tops[i]!;
    this.current = view;
    this.jumped = this.element.scrollTop;
    this.paint();
  }
}
