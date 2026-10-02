/** diffr in the browser: a GitHub PR or comparison, diffed by the wasm engine on this machine. */
import "./fonts";
import "./style.css";
import { diff, engineStats, onEngineChange, release } from "./engine";
import { fileText, loadChange, parseTarget, setToken, targetPath, token, type Change, type ChangedFile, type Preview, type Target } from "./github";
import { icons, logo, mountSprite } from "./icons";
import type { Layout } from "./model";
import { escape, FileView, type FileHost } from "./view/file";
import { Sidebar, type Timing } from "./view/sidebar";
import { Viewer } from "./view/viewer";

type ThemePreference = "system" | "dark" | "light";

const EXAMPLES = ["devdotfast/whiteboard/pull/837", "oven-sh/bun/pull/30412", "nodejs/node/pull/59805", "ghostty-org/ghostty/pull/12291"];
/** Files diffr works on at once: both sides fetched, then diffed. */
const DIFFS = 8;

const app = document.querySelector<HTMLElement>("#app")!;
const darkQuery = matchMedia("(prefers-color-scheme: dark)");
let preference = read<ThemePreference>("diffr.theme", ["system", "dark", "light"], "system");
let layout = read<Layout>("diffr.layout", ["split", "unified"], "split");
/** Too narrow for two columns: unified, whatever the reader picked for wider screens. */
const narrowQuery = matchMedia("(max-width: 760px)");
const effectiveLayout = (): Layout => (narrowQuery.matches ? "unified" : layout);
narrowQuery.addEventListener("change", () => page?.repaint());
/** Bumped per navigation, so a slow load for an old page never paints over a new one. */
let generation = 0;
let page: ChangePage | undefined;

function read<T extends string>(key: string, values: T[], fallback: T): T {
  try {
    const value = localStorage.getItem(key) as T | null;
    return value && values.includes(value) ? value : fallback;
  } catch {
    return fallback;
  }
}

function store(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Applied, just not remembered.
  }
}

const theme = (): "dark" | "light" => (preference === "system" ? (darkQuery.matches ? "dark" : "light") : preference);

function applyTheme() {
  document.documentElement.dataset.theme = theme();
  page?.repaint();
}
darkQuery.addEventListener("change", applyTheme);

/** The URL bar's text for this page. */
const githubUrl = (target: Target) => `https://github.com${targetPath(target)}`;

function tokenSection(compact = false) {
  const saved = !!token();
  return `<form class="token${compact ? " compact" : ""}" data-token-form>
      <div class="token-title">${icons.github}<span>Private GitHub access</span><span class="pill">${saved ? "Saved" : "Optional"}</span></div>
      <p>Create a <a href="https://github.com/settings/personal-access-tokens/new" target="_blank" rel="noreferrer">fine-grained PAT</a>
        to view private diffs, or a <a href="https://github.com/settings/tokens/new?scopes=repo&description=diffr" target="_blank" rel="noreferrer">classic token</a>
        with repo scope. Saved only in localStorage, sent only to GitHub.</p>
      <div class="token-row">
        <input type="password" name="token" placeholder="${saved ? "A token is saved; paste another to replace it" : "Paste token"}" autocomplete="off" spellcheck="false" aria-label="GitHub token">
        ${saved ? `<button type="button" class="button ghost" data-forget>Forget</button>` : ""}
        <button class="button">Save</button>
      </div>
    </form>`;
}

function bindToken(root: HTMLElement, done: () => void) {
  const form = root.querySelector<HTMLFormElement>("[data-token-form]");
  if (!form) return;
  form.addEventListener("submit", (event) => {
    event.preventDefault();
    const value = form.querySelector<HTMLInputElement>("input[name=token]")!.value.trim();
    if (!value) return;
    setToken(value);
    done();
  });
  form.querySelector("[data-forget]")?.addEventListener("click", () => {
    setToken(null);
    done();
  });
}

function home(error?: string) {
  page?.destroy();
  page = undefined;
  document.title = "diffr";
  app.className = "home";
  const host = location.host || "diffr";
  app.innerHTML = `
    <section class="landing">
      <h2>${logo(24)}<span>diffr</span></h2>
      <p class="lede">Semantic diffs of GitHub pull requests and comparisons, computed in your browser. diffr parses
        both sides with tree-sitter, pairs what moved, and folds what did not change. Replace <code>github.com</code>
        with <code>${escape(host)}</code> in any pull request URL.</p>
      <div class="demo">
        <code class="removed"><span>- github</span>.com/org/repo/pull/number</code>
        <code class="added"><span>+ ${escape(host)}</span>/org/repo/pull/number</code>
      </div>
      ${error ? `<p class="landing-error" role="alert">${escape(error)}</p>` : ""}
      <div class="card">
        <form class="go" data-go>
          <input name="target" placeholder="https://github.com/org/repo/pull/123" autocomplete="off" spellcheck="false" aria-label="Pull request or comparison URL" autofocus>
          <button class="icon-button" title="View diff">${icons.arrow}</button>
        </form>
        ${tokenSection()}
      </div>
      <h3>Enter a URL above, or use one of these:</h3>
      <ul class="examples">${EXAMPLES.map((path) => `<li>${icons.arrow}<a href="/${path}" data-link><span>https://github.com/</span>${path}</a></li>`).join("")}</ul>
      <p class="note">Nothing leaves this page except requests to GitHub. The diff engine is diffr itself, compiled to
        WebAssembly, running on your machine in up to ${engineStats.workers} workers.
        Comparisons work too, as <code>owner/repo/compare/base...head</code>.</p>
      <hr>
      <p class="credit">Built on <a href="https://github.com/devdotfast/diffr" target="_blank" rel="noreferrer">diffr</a>,
        with <a href="https://trees.software" target="_blank" rel="noreferrer">Trees</a> and
        <a href="https://diffs.com" target="_blank" rel="noreferrer">Diffs</a> from The Pierre Computer Company.</p>
    </section>`;
  app.querySelector<HTMLFormElement>("[data-go]")!.addEventListener("submit", (event) => {
    event.preventDefault();
    go(new FormData(event.target as HTMLFormElement).get("target") as string);
  });
  bindToken(app, () => home());
}

function go(input: string) {
  const target = parseTarget(input);
  if (!target) return home("Enter a pull request (github.com/owner/repo/pull/123) or a comparison (github.com/owner/repo/compare/base...head).");
  navigate(targetPath(target));
}

/** A pull request or comparison: the top bar, the file tree, and the files. */
/**
 * A rewrite whose structural match may run to the graph limit: diffr's matching grows with the
 * removed lines times the added ones, and a file GitHub would not patch has no counts to go by.
 */
function mayRunLarge(file: ChangedFile) {
  if (file.status !== "modified" && file.status !== "renamed") return false;
  return file.additions * file.deletions >= LARGE_REWRITE || !file.patch;
}

/** Changed lines, removed times added, past which a file is diffed on its own; 137 × 195 reached 6 million. */
const LARGE_REWRITE = 5000;

class ChangePage {
  readonly run = ++generation;
  views: FileView[] = [];
  sidebar = new Sidebar();
  viewer: Viewer;
  timing: Timing = { start: performance.now() };
  change?: Change;
  private statsFrame = 0;
  private closedAll = false;
  private keys = (event: KeyboardEvent) => this.onKey(event);

  constructor(readonly target: Target) {
    app.className = "change";
    app.innerHTML = `
      <header class="topbar">
        <a class="brand" href="/" data-link title="diffr">${logo(24)}</a>
        <form class="target" data-go>
          <input name="target" value="${escape(githubUrl(target))}" autocomplete="off" spellcheck="false" aria-label="Pull request or comparison URL">
          <button type="button" class="icon-button" data-action="clear" title="Clear">${icons.close}</button>
        </form>
        <span class="grow"></span>
        <div class="actions">
          <a class="icon-button" href="${escape(githubUrl(target))}" target="_blank" rel="noreferrer" title="Open on GitHub">${icons.external}</a>
          <span class="divider"></span>
          <button class="icon-button" data-action="layout"></button>
          <button class="icon-button" data-action="collapse"></button>
          <button class="icon-button" data-action="theme"></button>
          <button class="icon-button" data-action="token" title="GitHub access">${icons.settings}</button>
        </div>
      </header>
      <main class="viewer"><div class="status" data-viewer-status>${icons.loading}<h2>Loading ${escape(targetPath(target).slice(1))}</h2><p>Reading the change from GitHub…</p></div></main>
      <dialog class="dialog" data-token-dialog></dialog>`;
    app.insertBefore(this.sidebar.element, app.querySelector(".viewer"));
    this.viewer = new Viewer(app.querySelector<HTMLElement>(".viewer")!);
    this.viewer.onCurrent = (view) => this.sidebar.follow(view.path);
    this.sidebar.onSelect = (path) => {
      const view = this.views.find((v) => v.path === path);
      if (view) this.viewer.scrollTo(view);
    };
    this.sidebar.onFilter = (paths) => this.viewer.setVisible(this.views.filter((view) => paths.has(view.path)));
    app.querySelector(".topbar")!.addEventListener("click", (event) => this.onAction(event as MouseEvent));
    app.querySelector<HTMLFormElement>("[data-go]")!.addEventListener("submit", (event) => {
      event.preventDefault();
      go(new FormData(event.target as HTMLFormElement).get("target") as string);
    });
    addEventListener("keydown", this.keys);
    onEngineChange(() => this.scheduleStats());
    this.paintActions();
    this.scheduleStats();
  }

  destroy() {
    removeEventListener("keydown", this.keys);
  }

  private get live() {
    return this.run === generation;
  }

  async load() {
    let change: Change;
    try {
      change = await loadChange(this.target, (preview) => this.live && this.show(preview));
    } catch (error) {
      if (!this.live) return;
      this.status("Could not load this change", error instanceof Error ? error.message : String(error), true);
      return;
    }
    if (!this.live) return;
    this.timing.listed = performance.now();
    this.show(change);
    this.change = change;
    this.pump();
  }

  /** What GitHub has listed so far: the first page early, then the whole change. */
  listing?: Preview;

  /**
   * Show the listed files, each holding room for its diff until diffr has it. A later, longer listing
   * appends to the files already shown.
   */
  private show(listing: Preview) {
    const first = !this.listing;
    if (listing.files.length < (this.listing?.files.length ?? 0)) listing = { ...this.listing!, base: listing.base };
    this.listing = listing;
    if (listing.base && !this.change) {
      this.change = { ...listing, base: listing.base };
      queueMicrotask(() => this.pump());
    }
    if (first) {
      document.title = `${listing.title} · diffr`;
      this.host = {
        layout: effectiveLayout(), theme: theme(),
        changed: (view) => this.changed(view),
      };
      this.viewer.onNear = (views) => this.want(views);
    }
    if (listing.files.length === this.views.length && !first) return;
    for (let i = this.views.length; i < listing.files.length; i++) this.views.push(new FileView(listing.files[i]!, i, this.host!));
    try {
      this.sidebar.setFiles(listing.files);
    } catch (error) {
      // The tree is a convenience; the files still load without it.
      console.error(error);
    }
    if (!this.views.length) return this.status("No files changed", "This change has no file differences.");
    app.querySelector("[data-viewer-status]")?.remove();
    this.viewer.setViews(this.views);
    this.viewer.paint();
    this.timing.firstPaint ??= performance.now();
    this.scheduleStats();
  }

  /** Files near the viewport, nearest first: the ones diffr should work on next. */
  private wanted: FileView[] = [];
  private inflight = 0;
  private idleTimer = 0;

  private want(views: FileView[]) {
    this.wanted = views;
    this.pump();
  }

  /** Start diffing wanted files, a few at a time; files scrolled past before their turn are skipped. */
  private pump() {
    if (!this.live || !this.change) return;
    clearTimeout(this.idleTimer);
    for (const view of this.wanted) {
      if (this.inflight >= DIFFS) break;
      if (view.state === "waiting") void this.diffOne(this.change, view);
    }
    if (!this.inflight) {
      if (!this.timing.done && this.timing.firstDiff) this.settled();
      // Nothing to do for a while: let the engine's extra workers and their memory go.
      this.idleTimer = window.setTimeout(release, 10_000);
    }
  }

  private async diffOne(change: Change, view: FileView) {
    this.inflight++;
    view.markLoading();
    this.viewer.schedule();
    this.scheduleStats();
    try {
      const result = await this.diffFile(change, view);
      if (!this.live) return;
      view.diffMs = result.ms;
      view.setResult(result);
      this.timing.firstDiff ??= performance.now();
    } catch (error) {
      if (!this.live) return;
      view.setResult(undefined, error instanceof Error ? error.message : String(error));
    } finally {
      this.inflight--;
    }
    this.viewer.measure();
    this.scheduleStats();
    this.pump();
  }

  /** The files on screen at load are diffed: what the page reports as its load time. */
  private settled() {
    this.timing.done = performance.now();
    this.scheduleStats();
    (window as unknown as { diffrTiming: unknown }).diffrTiming = {
      listed: this.timing.listed! - this.timing.start,
      firstPaint: this.timing.firstPaint! - this.timing.start,
      firstDiff: this.timing.firstDiff! - this.timing.start,
      done: this.timing.done - this.timing.start,
      files: this.views.length,
      diffed: this.views.filter((view) => view.state === "done").length,
      diffMs: this.views.reduce((sum, view) => sum + view.diffMs, 0),
    };
  }

  private host?: FileHost;

  private async diffFile(change: Change, view: FileView) {
    const { file } = view;
    const lhsPath = file.previousPath ?? file.path;
    const [lhs, rhs] = await Promise.all([
      file.status === "added" ? undefined : fileText(change.target, change.base, lhsPath).then((text) => ({ path: lhsPath, text })),
      file.status === "deleted" ? undefined : fileText(change.target, change.head, file.path).then((text) => ({ path: file.path, text })),
    ]);
    return diff(file.status, lhs, rhs, mayRunLarge(file));
  }

  private status(title: string, detail: string, error = false) {
    const viewer = app.querySelector(".viewer")!;
    viewer.querySelector("[data-viewer-status]")?.remove();
    const status = document.createElement("div");
    status.className = "status";
    status.dataset.viewerStatus = "";
    status.toggleAttribute("data-error", error);
    status.innerHTML = `${error ? icons.info : icons.files}<h2>${escape(title)}</h2><p>${escape(detail)}</p>`
      + (error && !token() ? `<button class="button" data-action="token">Add a GitHub token</button>` : "");
    status.querySelector("[data-action=token]")?.addEventListener("click", () => this.openToken());
    viewer.append(status);
  }

  private changed(_view: FileView) {
    this.viewer.measure();
    this.scheduleStats();
  }

  private scheduleStats() {
    if (this.statsFrame) return;
    this.statsFrame = requestAnimationFrame(() => {
      this.statsFrame = 0;
      this.sidebar.updateStats(this.views, this.listing);
      this.sidebar.updateEngine(this.timing, this.views);
    });
  }

  repaint() {
    if (this.host) {
      this.host.layout = effectiveLayout();
      this.host.theme = theme();
    }
    for (const view of this.views) view.invalidate();
    this.viewer.measure();
    this.viewer.paint();
    this.paintActions();
  }

  private paintActions() {
    const set = (action: string, html: string, title: string) => {
      const button = app.querySelector<HTMLElement>(`[data-action=${action}]`)!;
      button.innerHTML = html;
      button.title = title;
    };
    set("layout", layout === "split" ? icons.split : icons.unified, layout === "split" ? "Switch to unified view" : "Switch to split view");
    set("collapse", this.closedAll ? icons.expand : icons.collapse, this.closedAll ? "Expand all files" : "Collapse all files");
    set("theme", icons.theme, `Theme: ${preference} (click to change)`);
  }

  private onAction(event: MouseEvent) {
    const action = (event.target as HTMLElement).closest<HTMLElement>("[data-action]")?.dataset.action;
    if (!action) return;
    if (action === "clear") {
      const input = app.querySelector<HTMLInputElement>(".target input")!;
      input.value = "";
      input.focus();
    } else if (action === "layout") {
      layout = layout === "split" ? "unified" : "split";
      store("diffr.layout", layout);
      this.repaint();
    } else if (action === "collapse") {
      this.closedAll = !this.closedAll;
      for (const view of this.views) {
        view.closed = this.closedAll;
        view.invalidate();
      }
      this.viewer.measure();
      this.paintActions();
    } else if (action === "theme") {
      preference = preference === "system" ? (theme() === "dark" ? "light" : "dark") : preference === "dark" ? "light" : "system";
      store("diffr.theme", preference);
      applyTheme();
    } else if (action === "token") this.openToken();
  }

  private openToken() {
    const dialog = app.querySelector<HTMLDialogElement>("[data-token-dialog]")!;
    dialog.innerHTML = `${tokenSection(true)}<button class="icon-button dialog-close" title="Close">${icons.close}</button>`;
    dialog.querySelector(".dialog-close")!.addEventListener("click", () => dialog.close());
    bindToken(dialog, () => {
      dialog.close();
      route();
    });
    dialog.addEventListener("click", (event) => {
      if (event.target === dialog) dialog.close();
    });
    dialog.showModal();
  }

  private onKey(event: KeyboardEvent) {
    if (event.key === "F2" || event.key === "F3") {
      event.preventDefault();
      this.sidebar.togglePanel(event.key === "F2" ? "stats" : "engine");
    } else if (event.key === "/" && !typing(event)) {
      event.preventDefault();
      this.sidebar.openSearch();
    }
  }
}

/** The key goes to a text field, perhaps one inside a shadow root such as the tree's search. */
function typing(event: KeyboardEvent) {
  const target = event.composedPath()[0];
  return target instanceof Element && !!target.closest("input, textarea, [contenteditable]");
}

function route() {
  const target = parseTarget(location.pathname);
  if (!target) {
    generation++;
    return home();
  }
  page?.destroy();
  page = new ChangePage(target);
  // For measuring from the console: timings, views, and the viewer.
  (window as unknown as { diffr: ChangePage }).diffr = page;
  void page.load();
}

function navigate(path: string) {
  if (path !== location.pathname) history.pushState(null, "", path);
  route();
}

document.addEventListener("click", (event) => {
  const link = (event.target as HTMLElement).closest<HTMLAnchorElement>("a[data-link]");
  if (!link || event.metaKey || event.ctrlKey || event.shiftKey) return;
  event.preventDefault();
  navigate(new URL(link.href).pathname);
});
addEventListener("popstate", route);

mountSprite();
applyTheme();
route();
