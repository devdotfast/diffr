/**
 * The left column: the changed files as a tree (@pierre/trees, Apache-2.0), then the diff stats and
 * engine panels.
 */
import { FileTree, type GitStatus } from "@pierre/trees";
import { activeWorkers, engineStats } from "../engine";
import type { ChangedFile } from "../github";
import { icons } from "../icons";
import type { FileView } from "./file";

const gitStatus: Record<ChangedFile["status"], GitStatus> = {
  added: "added", deleted: "deleted", modified: "modified", renamed: "renamed", copied: "added",
};

const statusNames: [GitStatus, string][] = [["added", "Added"], ["modified", "Modified"], ["renamed", "Renamed"], ["deleted", "Deleted"]];

const treeCss = `
  [data-file-tree-search-container][data-open='false'] { display: none; }
  [data-file-tree-search-container] {
    padding-bottom: 12px; margin-bottom: 12px; margin-right: 4px;
    border-bottom: 1px solid var(--trees-theme-sidebar-border); padding-inline-start: 1px; padding-inline-end: 5px;
  }
  [data-file-tree-sticky-overlay-content] { box-shadow: 0 2px 3px -4px rgb(0 0 0 / 1); }
  [data-item-type='folder'] { font-weight: 500; color: color-mix(in lab, var(--trees-folder-mix) 25%, var(--trees-fg)); }
  [data-item-type='folder'] [data-item-section='git'] { display: none; }
`;

const number = (n: number) => n.toLocaleString("en-US");
const seconds = (ms: number) => (ms >= 1000 ? `${(ms / 1000).toFixed(2)} s` : `${Math.round(ms)} ms`);

export interface Timing {
  start: number;
  listed?: number;
  firstPaint?: number;
  firstDiff?: number;
  done?: number;
}

export class Sidebar {
  readonly element: HTMLElement;
  private tree?: FileTree;
  private treeHost: HTMLElement;
  private statsRows: HTMLElement;
  private engineRows: HTMLElement;
  private filterMenu: HTMLElement;
  private files: ChangedFile[] = [];
  /** Statuses the reader hid with the filter. */
  hidden = new Set<GitStatus>();
  private syncing = false;
  onSelect?: (path: string) => void;
  onFilter?: (visible: Set<string>) => void;

  constructor() {
    this.element = document.createElement("aside");
    this.element.className = "sidebar";
    this.element.innerHTML = `
      <div class="sidebar-head">
        <button class="tab" aria-pressed="true" title="Files">${icons.files}</button>
        <span class="sidebar-count" data-count></span>
        <span class="grow"></span>
        <button class="tab" data-action="search" title="Search files (/)">${icons.search}</button>
        <button class="tab" data-action="filter" title="Filter by Git status" aria-haspopup="menu">${icons.filter}</button>
        <div class="menu" role="menu" hidden></div>
      </div>
      <div class="tree"></div>
      <section class="panel" data-panel="stats">
        <button class="panel-head" data-toggle="stats">${icons.stats}<span>Diff Stats <span class="key">(F2)</span></span></button>
        <div class="panel-rows" data-rows></div>
      </section>
      <section class="panel" data-panel="engine">
        <button class="panel-head" data-toggle="engine">${icons.engine}<span>Engine <span class="key">(F3)</span></span></button>
        <div class="panel-rows" data-rows></div>
      </section>`;
    this.treeHost = this.element.querySelector(".tree")!;
    this.statsRows = this.element.querySelector("[data-panel=stats] [data-rows]")!;
    this.engineRows = this.element.querySelector("[data-panel=engine] [data-rows]")!;
    this.filterMenu = this.element.querySelector(".menu")!;
    for (const name of ["stats", "engine"]) {
      const open = readPanel(name);
      this.element.querySelector(`[data-panel=${name}]`)!.toggleAttribute("data-open", open);
    }
    this.element.addEventListener("click", (event) => this.onClick(event));
  }

  setFiles(files: ChangedFile[]) {
    this.files = files;
    const rank = new Map<string, number>();
    files.forEach((file, index) => {
      const parts = file.path.split("/");
      for (let i = 1; i <= parts.length; i++) {
        const prefix = parts.slice(0, i).join("/");
        if (!rank.has(prefix)) rank.set(prefix, index);
      }
    });
    this.tree?.cleanUp();
    this.treeHost.replaceChildren();
    this.tree = new FileTree({
      paths: files.map((file) => file.path),
      gitStatus: files.map((file) => ({ path: file.path, status: gitStatus[file.status] })),
      initialExpansion: "open",
      flattenEmptyDirectories: true,
      // GitHub's order, which is the viewer's, so the tree and the files read the same way down: a
      // folder sits where its first file does.
      sort: (left, right) => (rank.get(left.path.replace(/\/$/, "")) ?? 0) - (rank.get(right.path.replace(/\/$/, "")) ?? 0),
      search: true,
      fileTreeSearchMode: "hide-non-matches",
      density: 0.8,
      itemHeight: 24,
      icons: { set: "standard", colored: true },
      unsafeCSS: treeCss,
      onSelectionChange: (paths) => {
        if (this.syncing) return;
        const path = paths.at(-1);
        if (path && files.some((file) => file.path === path)) this.onSelect?.(path);
      },
    });
    this.tree.render({ fileTreeContainer: this.treeHost });
    this.element.querySelector("[data-count]")!.textContent = `${number(files.length)} file${files.length === 1 ? "" : "s"}`;
    const present = new Set(files.map((file) => gitStatus[file.status]));
    this.filterMenu.innerHTML = statusNames
      .filter(([status]) => present.has(status))
      .map(([status, name]) => `<label class="menu-item" data-status="${status}"><input type="checkbox" ${this.hidden.has(status) ? "" : "checked"} data-filter="${status}"><span class="dot"></span>${name}</label>`)
      .join("");
  }

  /** Highlight the file the viewer is showing, without scrolling the viewer back to it. */
  follow(path: string) {
    if (!this.tree) return;
    this.syncing = true;
    for (const selected of this.tree.getSelectedPaths()) if (selected !== path) this.tree.getItem(selected)?.deselect();
    this.tree.getItem(path)?.select();
    this.tree.scrollToPath(path, { offset: "nearest" });
    this.syncing = false;
  }

  openSearch() {
    this.tree?.openSearch();
  }

  /** `totals`: GitHub's own counts for the change, which cover files too large for it to patch. */
  updateStats(views: FileView[], totals?: { additions?: number; deletions?: number }) {
    let added = 0, removed = 0, diffed = 0;
    for (const view of views) {
      const stats = view.stats;
      if (stats) {
        added += stats.added;
        removed += stats.removed;
      }
      if (view.state === "done") diffed++;
    }
    // Until every file is diffed, GitHub's totals are the complete ones.
    if (diffed < views.length && totals?.additions !== undefined) {
      added = totals.additions;
      removed = totals.deletions ?? removed;
    }
    this.statsRows.innerHTML = row("Files", number(views.length))
      + row("Additions", number(added), "added")
      + row("Deletions", number(removed), "removed")
      + row("Diffed by diffr", `${number(diffed)} / ${number(views.length)}`, "", "Files diffr has diffed; the rest are diffed as they come into view");
  }

  updateEngine(timing: Timing, views: FileView[]) {
    const diffed = views.filter((view) => view.diffMs);
    const total = diffed.reduce((sum, view) => sum + view.diffMs, 0);
    const slowest = diffed.reduce<FileView | undefined>((a, b) => (!a || b.diffMs > a.diffMs ? b : a), undefined);
    const since = (at?: number) => (at ? seconds(at - timing.start) : "…");
    const busy = views.filter((view) => view.state === "loading").length;
    this.engineRows.innerHTML = row("Engine", `diffr wasm × ${activeWorkers()}`)
      + row("Engine ready", engineStats.ready ? seconds(engineStats.ready) : "loading…")
      + row("Files listed", since(timing.listed))
      + row("Files shown", since(timing.firstPaint))
      + row("First diff", since(timing.firstDiff))
      + row("Screen diffed", since(timing.done))
      + row("Diffing now", number(busy))
      + row("Engine time", diffed.length ? seconds(total) : "…")
      + (slowest ? row("Slowest", seconds(slowest.diffMs), "", slowest.path) : "");
  }

  private onClick(event: MouseEvent) {
    const target = event.target as HTMLElement;
    const toggle = target.closest<HTMLElement>("[data-toggle]");
    if (toggle) return this.togglePanel(toggle.dataset.toggle!);
    const action = target.closest<HTMLElement>("[data-action]")?.dataset.action;
    if (action === "search") return this.openSearch();
    if (action === "filter") {
      this.filterMenu.hidden = !this.filterMenu.hidden;
      if (!this.filterMenu.hidden)
        setTimeout(() => document.addEventListener("click", (e) => {
          if (!this.filterMenu.contains(e.target as Node)) this.filterMenu.hidden = true;
        }, { once: true }));
      return;
    }
    const filter = target.closest<HTMLInputElement>("input[data-filter]");
    if (filter) {
      const status = filter.dataset.filter as GitStatus;
      if (filter.checked) this.hidden.delete(status);
      else this.hidden.add(status);
      const visible = this.files.filter((file) => !this.hidden.has(gitStatus[file.status]));
      this.tree?.resetPaths(visible.map((file) => file.path));
      this.onFilter?.(new Set(visible.map((file) => file.path)));
      this.element.querySelector("[data-action=filter]")!.toggleAttribute("data-active", this.hidden.size > 0);
    }
  }

  togglePanel(name: string) {
    const panel = this.element.querySelector(`[data-panel=${name}]`)!;
    const open = !panel.hasAttribute("data-open");
    panel.toggleAttribute("data-open", open);
    try {
      localStorage.setItem(`diffr.panel.${name}`, open ? "1" : "0");
    } catch {
      // Remembered for this page only.
    }
  }
}

function readPanel(name: string) {
  try {
    return localStorage.getItem(`diffr.panel.${name}`) !== "0";
  } catch {
    return true;
  }
}

function row(label: string, value: string, kind = "", title = "") {
  return `<div class="panel-row"${title ? ` title="${title.replace(/"/g, "&quot;")}"` : ""}><span>${label}</span><span class="value ${kind}">${value}</span></div>`;
}
