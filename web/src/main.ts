/** diffr in the browser: a GitHub PR or comparison, diffed by the wasm engine on this machine. */
import "./style.css";
import { add, blockBar, zero, type LineCounts } from "../../tui/packages/hunk/src/diffr/counts";
import { defaultCollapsed } from "../../tui/packages/hunk/src/diffr/regions";
import { dark, light, rowsForFile, type Layout, type Palette } from "../../tui/packages/hunk/src/diffr/rows";
import type { DiffFile, FileEvent } from "../../tui/packages/hunk/src/diffr/wire";
import { diff } from "./engine";
import { fileText, loadChange, parseTarget, setToken, targetPath, token, type Change, type ChangedFile, type Target } from "./github";
import { escape, paletteVars, renderRows } from "./render";

const EXAMPLE = "devdotfast/whiteboard/pull/837";
const LAYOUT_KEY = "diffr.layout";

interface FileView {
  file: ChangedFile;
  section: HTMLElement;
  state: "loading" | "done" | "failed";
  event?: FileEvent;
  error?: string;
  collapsed: Set<number>;
  /** A file diffr hides by default (generated, vendored) stays behind "Load diff" until asked. */
  revealed: boolean;
}

const app = document.querySelector<HTMLElement>("#app")!;
const darkQuery = matchMedia("(prefers-color-scheme: dark)");
let theme: Palette = darkQuery.matches ? dark : light;
let layout: Layout = readLayout();
let views: FileView[] = [];
/** Bumped per navigation, so a slow load for an old page never paints over a new one. */
let generation = 0;

function readLayout(): Layout {
  try {
    return localStorage.getItem(LAYOUT_KEY) === "unified" ? "unified" : "split";
  } catch {
    return "split";
  }
}

function applyTheme() {
  document.documentElement.setAttribute("style", paletteVars(theme));
  document.documentElement.dataset.theme = theme.isLight ? "light" : "dark";
}

darkQuery.addEventListener("change", () => {
  theme = darkQuery.matches ? dark : light;
  applyTheme();
  views.forEach(paintFile);
});

function shell(content: string) {
  const current = location.pathname === "/" ? "" : `github.com${location.pathname}`;
  app.innerHTML = `
    <header class="bar">
      <a class="brand" href="/" data-link>diffr</a>
      <form class="go" id="go">
        <input name="target" placeholder="github.com/owner/repo/pull/123 or owner/repo/compare/main...branch"
          value="${escape(current)}" autocomplete="off" spellcheck="false" aria-label="Pull request or comparison">
        <button>View</button>
      </form>
      <div class="tools">
        <div class="segmented" role="group" aria-label="Layout">
          <button data-layout="split" aria-pressed="${layout === "split"}">Split</button>
          <button data-layout="unified" aria-pressed="${layout === "unified"}">Unified</button>
        </div>
        <button id="token-button" class="ghost">${token() ? "Token ✓" : "Token"}</button>
      </div>
    </header>
    <dialog id="token-dialog">
      <form method="dialog" class="token-form">
        <h2>GitHub token</h2>
        <p>Public repositories need none. A token lifts GitHub's limit of 60 requests an hour and opens private
          repositories. It is kept in this browser's localStorage and sent only to api.github.com.</p>
        <input type="password" name="token" placeholder="github_pat_…" autocomplete="off" spellcheck="false">
        <div class="actions">
          <button value="clear" class="ghost">Forget token</button>
          <button value="cancel" class="ghost">Cancel</button>
          <button value="save">Save</button>
        </div>
      </form>
    </dialog>
    ${content}`;
  app.querySelector<HTMLFormElement>("#go")!.addEventListener("submit", (event) => {
    event.preventDefault();
    const input = new FormData(event.target as HTMLFormElement).get("target") as string;
    const target = parseTarget(input);
    if (!target) return alert("Enter a pull request (owner/repo/pull/123) or a comparison (owner/repo/compare/base...head).");
    navigate(targetPath(target));
  });
  for (const button of app.querySelectorAll<HTMLButtonElement>("[data-layout]"))
    button.addEventListener("click", () => {
      layout = button.dataset.layout as Layout;
      try {
        localStorage.setItem(LAYOUT_KEY, layout);
      } catch {
        // Not remembered; still applied.
      }
      for (const other of app.querySelectorAll<HTMLButtonElement>("[data-layout]"))
        other.setAttribute("aria-pressed", String(other === button));
      views.forEach(paintFile);
    });
  const dialog = app.querySelector<HTMLDialogElement>("#token-dialog")!;
  app.querySelector("#token-button")!.addEventListener("click", () => dialog.showModal());
  dialog.addEventListener("close", () => {
    const value = (dialog.querySelector<HTMLInputElement>("input[name=token]")!).value.trim();
    if (dialog.returnValue === "save" && value) setToken(value);
    else if (dialog.returnValue === "clear") setToken(null);
    else return;
    route();
  });
}

function home() {
  shell(`
    <main class="home">
      <h1>Semantic diffs of GitHub changes, computed in your browser</h1>
      <p>diffr parses both sides with tree-sitter and diffs the syntax, folding what did not change. Everything runs
        here: files come straight from GitHub to this page, and nothing is sent anywhere else.</p>
      <p>Try <a href="/${EXAMPLE}" data-link>${EXAMPLE}</a>, or paste a pull request above.</p>
    </main>`);
}

const statusLetter: Record<ChangedFile["status"], string> = {
  added: "A", deleted: "D", modified: "M", renamed: "R", copied: "C",
};

function counts(view: FileView): LineCounts | undefined {
  const d = view.event?.diff;
  return d?.type === "text" ? d.stats.visible : undefined;
}

function bar(stats: LineCounts | undefined): string {
  if (!stats) return "";
  return `<span class="stats"><span class="added">+${stats.added}</span> <span class="removed">−${stats.removed}</span>`
    + `<span class="blocks">${blockBar(stats).map((b) => `<i class="${b}"></i>`).join("")}</span></span>`;
}

function fileId(index: number) {
  return `file-${index}`;
}

function paintSidebar() {
  const list = document.querySelector("#files");
  if (!list) return;
  list.innerHTML = views
    .map((view, index) => {
      const name = view.file.path.split("/").pop()!;
      const dir = view.file.path.slice(0, -name.length);
      const stats = counts(view);
      const state = view.state === "loading" ? `<span class="spinner" aria-label="Diffing"></span>`
        : view.state === "failed" ? `<span class="removed">!</span>`
        : stats ? `<span class="added">+${stats.added}</span><span class="removed">−${stats.removed}</span>` : "";
      return `<li><a href="#${fileId(index)}" title="${escape(view.file.path)}">`
        + `<span class="status status-${view.file.status}">${statusLetter[view.file.status]}</span>`
        + `<span class="name"><span class="dir">${escape(dir)}</span>${escape(name)}</span>`
        + `<span class="count">${state}</span></a></li>`;
    })
    .join("");
  const total = views.reduce((sum, view) => add(sum, counts(view) ?? zero), zero);
  const done = views.filter((view) => view.state !== "loading").length;
  document.querySelector("#summary")!.innerHTML = `${views.length} file${views.length === 1 ? "" : "s"}`
    + (done < views.length ? ` · diffing ${done}/${views.length}` : "") + ` ${bar(total)}`;
}

function paintFile(view: FileView) {
  const index = views.indexOf(view);
  const { file, event } = view;
  const d = event?.diff;
  const path = file.previousPath && file.previousPath !== file.path
    ? `${escape(file.previousPath)} → ${escape(file.path)}` : escape(file.path);
  const notes: string[] = [];
  if (d?.type === "text" && d.stats.fallback)
    notes.push(`<span class="note" title="${escape(d.stats.fallback.message)}">line diff</span>`);
  let body: string;
  if (view.state === "loading") body = `<div class="message"><span class="spinner"></span> Diffing…</div>`;
  else if (view.state === "failed" || event?.error)
    body = `<div class="message error">${escape(view.error ?? event?.error?.message ?? "diffr failed")}</div>`;
  else if (!d) body = `<div class="message">No diff</div>`;
  else if (event!.visibility.collapsed && !view.revealed)
    body = `<div class="message"><button class="link" data-reveal>Load diff</button> ${escape(event!.visibility.label || "Hidden by default")}</div>`;
  else if (d.type === "binary") body = `<div class="message">Binary file</div>`;
  else {
    const rows = rowsForFile(event as DiffFile, index, layout, theme, view.collapsed);
    const html = renderRows(rows, layout);
    body = html ? `<div class="rows ${layout}">${html}</div>` : `<div class="message">No changes to show</div>`;
  }
  const foldable = d?.type === "text" && !(event!.visibility.collapsed && !view.revealed);
  view.section.innerHTML = `
    <header class="file-header">
      <span class="status status-${file.status}">${statusLetter[file.status]}</span>
      <span class="path">${path}</span>${notes.join("")}
      <span class="spacer"></span>
      ${foldable ? `<button class="ghost small" data-expand>Expand all</button><button class="ghost small" data-reset>Fold</button>` : ""}
      ${bar(counts(view))}
    </header>
    ${body}`;
  view.section.id = fileId(index);
}

function onFileClick(view: FileView, event: MouseEvent) {
  const target = event.target as HTMLElement;
  const fold = target.closest<HTMLElement>("[data-fold]");
  if (fold) {
    const id = Number(fold.dataset.fold);
    if (view.collapsed.has(id)) view.collapsed.delete(id);
    else view.collapsed.add(id);
  } else if (target.closest("[data-reveal]")) view.revealed = true;
  else if (target.closest("[data-expand]")) view.collapsed.clear();
  else if (target.closest("[data-reset]") && view.event?.diff?.type === "text")
    view.collapsed = defaultCollapsed(view.event.diff);
  else return;
  paintFile(view);
}

/** Fetch both sides of a file and diff them. */
async function diffFile(change: Change, view: FileView) {
  const { file } = view;
  const target = change.target;
  const lhsPath = file.previousPath ?? file.path;
  const [lhs, rhs] = await Promise.all([
    file.status === "added" ? undefined : fileText(target, change.base, lhsPath).then((text) => ({ path: lhsPath, text })),
    file.status === "deleted" ? undefined : fileText(target, change.head, file.path).then((text) => ({ path: file.path, text })),
  ]);
  return diff(file.status, lhs, rhs);
}

async function show(target: Target) {
  const run = ++generation;
  views = [];
  shell(`<main class="loading-page"><span class="spinner"></span> Loading ${escape(targetPath(target).slice(1))} from GitHub…</main>`);
  let change: Change;
  try {
    change = await loadChange(target);
  } catch (error) {
    if (run !== generation) return;
    shell(`<main class="home"><p class="error">${escape(error instanceof Error ? error.message : String(error))}</p></main>`);
    return;
  }
  if (run !== generation) return;
  document.title = `${change.title} · diffr`;
  shell(`
    <div class="layout">
      <nav class="sidebar"><div id="summary" class="summary"></div><ul id="files"></ul></nav>
      <main class="change">
        <div class="change-header">
          <h1>${escape(change.title)}</h1>
          <p><a href="${escape(change.url)}" target="_blank" rel="noreferrer">${escape(targetPath(target).slice(1))}</a>
            · <code>${change.base.slice(0, 7)}</code>…<code>${change.head.slice(0, 7)}</code></p>
        </div>
        <div id="diffs"></div>
      </main>
    </div>`);
  const diffs = document.querySelector("#diffs")!;
  views = change.files.map((file) => {
    const section = document.createElement("section");
    section.className = "file";
    diffs.append(section);
    const view: FileView = { file, section, state: "loading", collapsed: new Set(), revealed: false };
    section.addEventListener("click", (event) => onFileClick(view, event));
    return view;
  });
  views.forEach(paintFile);
  paintSidebar();
  if (!views.length) diffs.innerHTML = `<div class="message">No files changed</div>`;
  // Fetch a few files at a time; the worker diffs them in the order they arrive.
  const queue = [...views];
  const worker = async () => {
    for (let view = queue.shift(); view; view = queue.shift()) {
      try {
        view.event = await diffFile(change, view);
        if (view.event.diff?.type === "text") view.collapsed = defaultCollapsed(view.event.diff);
        view.state = "done";
      } catch (error) {
        view.state = "failed";
        view.error = error instanceof Error ? error.message : String(error);
      }
      if (run !== generation) return;
      paintFile(view);
      paintSidebar();
    }
  };
  await Promise.all(Array.from({ length: 6 }, worker));
}

function route() {
  const target = parseTarget(location.pathname);
  if (target) void show(target);
  else {
    generation++;
    views = [];
    document.title = "diffr";
    home();
  }
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

applyTheme();
route();
