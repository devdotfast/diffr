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
  private heights: number[] = [];
  private frame = 0;
  /** Called with the files on or near the screen, nearest first, whenever they change. */
  onNear?: (views: FileView[]) => void;
  private near = "";
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

  /**
   * Heights changed: a diff arrived, a fold toggled, the layout switched. The file being read stays
   * where it is on screen, however much the files above it grew or shrank.
   */
  measure() {
    const scroll = this.element.scrollTop;
    let anchor = -1, offset = 0;
    if (scroll > 0 && this.tops.length === this.visible.length)
      for (let i = 0; i < this.tops.length; i++)
        if (this.tops[i]! + this.heights[i]! > scroll) {
          anchor = i;
          offset = scroll - this.tops[i]!;
          break;
        }
    let top = 0;
    this.heights = this.visible.map((view) => view.height());
    this.tops = this.heights.map((height) => {
      const at = top;
      top += height + GAP;
      return at;
    });
    if (anchor >= 0) {
      const target = this.tops[anchor]! + Math.min(offset, this.heights[anchor]!);
      if (Math.abs(target - scroll) > 0.5) {
        this.element.scrollTop = target;
        if (this.jumped !== undefined) this.jumped = this.element.scrollTop;
      }
    }
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
    const onScreen: FileView[] = [], below: FileView[] = [], above: FileView[] = [];
    this.visible.forEach((view, i) => {
      const start = this.tops[i]!, end = start + view.height();
      if (end >= top - AHEAD && start <= top + height + AHEAD) {
        view.render(top - AHEAD - start, top + height + AHEAD - start);
        (end < top ? above : start > top + height ? below : onScreen).push(view);
      } else if (end < top - KEEP || start > top + height + KEEP) view.release();
      if (!current && end > top + 1) current = view;
    });
    const near = [...onScreen, ...below, ...above.reverse()];
    const key = near.map((view) => view.index).join(",");
    if (key !== this.near) {
      this.near = key;
      this.onNear?.(near);
    }
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
    this.jumped = this.element.scrollTop;
    if (view !== this.current) {
      this.current = view;
      this.onCurrent?.(view);
    }
    this.paint();
  }
}
