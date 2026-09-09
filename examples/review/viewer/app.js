import { foldLine } from "./inline-folds.mjs";
const $ = (id) => document.getElementById(id);
const escape = (s) =>
  String(s)
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;");
const encoder = new TextEncoder(),
  decoder = new TextDecoder();
let inlineFolds = [[], []];
let fixtures = [],
  current,
  source,
  tokens,
  closed = new Set(),
  expanded = new Set(),
  generation = 0;
const reviewed = new Set(
  JSON.parse(localStorage.getItem("diff-lab-reviewed") || "[]"),
);
const rowsOf = (range) => {
  const end = range.end.line + (range.end.byte_column > 0 ? 1 : 0);
  return Array.from(
    { length: Math.max(0, end - range.start.line) },
    (_, i) => range.start.line + i,
  );
};
function regions(c) {
  if (c.Paired) return [c.Paired.lhs, c.Paired.rhs];
  return c.Added ? [null, c.Added] : [c.Deleted, null];
}
function highlight(side, row, emphasize, from = 0, to = Infinity) {
  const bytes = encoder.encode(source[side][row] ?? "");
  let end = from,
    html = "";
  to = Math.min(to, bytes.length);
  for (const token of tokens[side].get(row) || []) {
    const p = token.pos,
      start = Math.max(end, p.start_col),
      stop = Math.min(to, p.end_col);
    if (stop <= start) continue;
    html += escape(decoder.decode(bytes.slice(end, start)));
    const [kind, value] = Object.entries(token.kind)[0];
    const atom = value.highlight?.Atom;
    const type =
      typeof atom === "object"
        ? "string"
        : { Keyword: "keyword", Type: "type", Comment: "comment" }[atom] || "";
    const novel = emphasize && (kind === "Novel" || kind === "NovelWord");
    html += `<span class="${type} ${novel ? "novel " + (side ? "added" : "deleted") : ""}">${escape(decoder.decode(bytes.slice(start, stop)))}</span>`;
    end = stop;
  }
  return html + escape(decoder.decode(bytes.slice(end, to)));
}
function sourceLine(side, row, semantic) {
  if (!semantic) return highlight(side, row, false);
  return foldLine(
    encoder.encode(source[side][row] ?? "").length,
    row,
    inlineFolds[side],
    closed,
  )
    .map((part) => {
      if (part.text) return highlight(side, row, true, ...part.text);
      const label = current.domain.folds[part.fold].placeholder;
      return `<button class="inline-fold" data-fold="${part.fold}" aria-label="${part.collapsed ? "Expand" : "Collapse"} ${escape(label)}" aria-expanded="${!part.collapsed}">${part.collapsed ? "⋯" : "▾"}</button>`;
    })
    .join("");
}
function line(l, r, kind, { semantic = false } = {}) {
  const side = r == null ? 0 : 1,
    row = r ?? l;
  const sign = { add: "+", del: "−", reindent: "↳" }[kind] || " ";
  const title =
    kind === "reindent"
      ? `Matched syntax. Base: ${source[0][l]}\nHead: ${source[1][r]}`
      : "";
  return `<div class="row ${kind}" data-left="${l ?? ""}" data-right="${r ?? ""}" title="${escape(title)}"><span class="num">${l == null ? "" : l + 1}</span><span class="num">${r == null ? "" : r + 1}</span><span class="sign">${sign}</span><span class="source">${sourceLine(side, row, semantic)}</span></div>`;
}
function renderGit() {
  let l = 0,
    r = 0,
    active = false,
    html = "";
  const patch = $("all").checked ? current.full_patch : current.patch;
  for (const text of patch.split("\n")) {
    const h = text.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
    if (h) {
      l = +h[1] - 1;
      r = +h[2] - 1;
      active = true;
      html += `<div class="gap">${escape(text)}</div>`;
      continue;
    }
    if (!active) continue;
    if (text.startsWith(" ")) html += line(l++, r++, "");
    else if (text.startsWith("-")) html += line(l++, null, "del");
    else if (text.startsWith("+")) html += line(null, r++, "add");
  }
  $("git").innerHTML = html || '<div class="empty">No textual changes</div>';
}
function visibleRows(domain, layout, showAll) {
  const selected = layout.baseline.map((rows) => new Set(rows));
  if (showAll) {
    source.forEach((lines, side) =>
      lines.forEach((_, row) => selected[side].add(row)),
    );
  }
  for (const context of domain.hunks.flatMap((hunk) => hunk.context)) {
    [context.lhs, context.rhs].forEach((range, side) => {
      if (!range) return;
      for (const row of rowsOf(range)) selected[side].add(row);
    });
  }
  domain.folds.forEach((fold, index) => {
    if (!expanded.has(index)) return;
    regions(fold.regions).forEach((range, side) => {
      if (!range) return;
      for (const row of rowsOf(range)) selected[side].add(row);
    });
  });
  return selected;
}

function indexFolds(folds) {
  // Keep every enclosing membership, including nested folds.
  const membership = [new Map(), new Map()];
  const inline = new Set();
  const inlineRanges = [[], []];
  folds.forEach((fold, index) => {
    const sides = regions(fold.regions);
    const isInline =
      fold.kind !== "Import" &&
      sides.some(
        (range) =>
          range && (range.start.byte_column > 0 || range.end.byte_column > 0),
      );
    if (isInline) inline.add(index);
    sides.forEach((range, side) => {
      if (!range) return;
      if (isInline) inlineRanges[side].push({ id: index, range });
      for (const row of rowsOf(range)) {
        if (!membership[side].has(row)) membership[side].set(row, []);
        membership[side].get(row).push(index);
      }
    });
  });
  return { membership, inline, inlineRanges };
}

function coversWholeRow(range, row) {
  return row != null && range && row > range.start.line && row < range.end.line;
}

function foldHeader(fold, index) {
  const description = regions(fold.regions)
    .map((range, side) =>
      range
        ? `${side ? "head" : "base"} ${range.start.line + 1}–${range.end.line + (range.end.byte_column > 0 ? 1 : 0)}`
        : null,
    )
    .filter(Boolean)
    .join(" ↔ ");
  return `<div class="fold"><button data-fold="${index}" aria-expanded="${!closed.has(index)}">${closed.has(index) ? "▸" : "▾"} ${escape(fold.placeholder)}</button><small>${description}</small></div>`;
}

function renderReview() {
  const { domain, layout } = current;
  const selected = visibleRows(domain, layout, $("all").checked);
  const { membership, inline, inlineRanges } = indexFolds(domain.folds);
  inlineFolds = inlineRanges;
  const size = (index) =>
    regions(domain.folds[index].regions).reduce(
      (n, r) => n + (r ? rowsOf(r).length : 0),
      0,
    );
  const reindented = new Set(layout.reindented.map((pair) => pair.join(":")));
  const seenFolds = new Set();
  let html = "",
    gap = false,
    removed = "",
    added = "";
  const flush = () => {
    html += removed + added;
    removed = "";
    added = "";
  };
  for (let [l, r] of layout.rows) {
    l = selected[0].has(l) ? l : null;
    r = selected[1].has(r) ? r : null;
    const folds = new Set(
      [...(membership[0].get(l) || []), ...(membership[1].get(r) || [])].sort(
        (a, b) => size(b) - size(a),
      ),
    );
    for (const index of folds) {
      if (
        !membership[0].get(l)?.includes(index) &&
        !membership[1].get(r)?.includes(index)
      )
        continue;
      if (inline.has(index)) {
        if (closed.has(index)) {
          const [lhsRange, rhsRange] = regions(domain.folds[index].regions);
          if (coversWholeRow(lhsRange, l)) l = null;
          if (coversWholeRow(rhsRange, r)) r = null;
        }
        continue;
      }
      if (!seenFolds.has(index)) {
        flush();
        if (gap) {
          html += '<div class="gap">⋯ unchanged context</div>';
          gap = false;
        }
        seenFolds.add(index);
        html += foldHeader(domain.folds[index], index);
      }
      if (closed.has(index)) {
        if (membership[0].get(l)?.includes(index)) l = null;
        if (membership[1].get(r)?.includes(index)) r = null;
      }
    }
    if (l == null && r == null) {
      if (!folds.size) {
        flush();
        gap = true;
      }
      continue;
    }
    if (gap) {
      flush();
      html += '<div class="gap">⋯ unchanged context</div>';
      gap = false;
    }
    if (
      l != null &&
      r != null &&
      (source[0][l] === source[1][r] || reindented.has(`${l}:${r}`))
    ) {
      flush();
      html += line(l, r, reindented.has(`${l}:${r}`) ? "reindent" : "", {
        semantic: true,
      });
    } else {
      if (l != null) removed += line(l, null, "del", { semantic: true });
      if (r != null) added += line(null, r, "add", { semantic: true });
    }
  }
  flush();
  if (gap) html += '<div class="gap">⋯ unchanged context</div>';
  $("review").classList.add("semantic");
  $("review").innerHTML =
    html || '<div class="empty">No syntactic changes</div>';
  $("stats").textContent =
    `${domain.folds.length} fold candidates · ${closed.size} collapsed`;
}
function navigation() {
  $("cases").innerHTML = fixtures
    .map(
      (f, i) =>
        `<button data-case="${f.id}" class="${current?.meta.id === f.id ? "active" : ""}"><strong>${reviewed.has(f.id) ? "✓" : String(i + 1).padStart(2, "0")} · ${escape(f.repo.split("/").at(-1))} #${f.pr}</strong><small>${escape(f.title)}</small></button>`,
    )
    .join("");
  $("progress").textContent = `${reviewed.size}/${fixtures.length}`;
  $("reviewed").textContent = reviewed.has(current?.meta.id)
    ? "✓ Reviewed"
    : "Mark reviewed";
}
function indexTokensByLine(positions) {
  const result = new Map();
  for (const position of positions) {
    const row = position.pos.line;
    if (!result.has(row)) result.set(row, []);
    result.get(row).push(position);
  }
  for (const rowTokens of result.values()) {
    rowTokens.sort(
      (a, b) =>
        a.pos.start_col - b.pos.start_col || a.pos.end_col - b.pos.end_col,
    );
  }
  return result;
}

async function select(id) {
  const request = ++generation;
  $("status").textContent = "Loading generated domain…";
  const response = await fetch(`data/${encodeURIComponent(id)}.json`);
  if (!response.ok) throw Error(`Unable to load ${id}`);
  const data = await response.json();
  if (request !== generation) return;
  current = data;
  expanded = new Set();
  closed = new Set(
    data.domain.folds.flatMap((f, i) =>
      f.kind === "Import" ? [i] : [],
    ),
  );
  source = [data.domain.lhs_src.Text, data.domain.rhs_src.Text].map(
    (s) => s.split("\n"),
  );
  tokens = [data.domain.lhs_positions, data.domain.rhs_positions].map(
    indexTokensByLine,
  );
  const m = data.meta;
  $("repo").innerHTML =
    `${escape(m.language)} / <a href="${escape(m.url)}" target="_blank" rel="noreferrer">${escape(m.repo)} #${m.pr} ↗</a>`;
  $("title").textContent = m.title;
  $("path").textContent = m.path;
  $("summary").textContent = m.summary;
  history.replaceState(null, "", `#${m.id}`);
  navigation();
  renderGit();
  renderReview();
  $("git").scrollTop = 0;
  $("review").scrollTop = 0;
  $("status").textContent =
    "Generated from pinned Git blobs. Blue rows are matched syntax with changed indentation. Review status stays in this browser.";
}
$("cases").onclick = (event) => {
  const button = event.target.closest("[data-case]");
  if (button) select(button.dataset.case).catch(fail);
};
$("review").onclick = (event) => {
  const button = event.target.closest("[data-fold]");
  if (button) {
    const id = +button.dataset.fold;
    if (closed.has(id)) {
      closed.delete(id);
      expanded.add(id);
    } else {
      closed.add(id);
      expanded.delete(id);
    }
    renderReview();
  }
};
$("wrap").onchange = () => {
  for (const id of ["git", "review"])
    $(id).classList.toggle("wrap", $("wrap").checked);
};
$("wrap").onchange();
$("all").onchange = () => {
  renderGit();
  renderReview();
};
$("expand").onclick = () => {
  closed.clear();
  expanded = new Set(current.domain.folds.map((_, i) => i));
  renderReview();
};
$("collapse").onclick = () => {
  closed = new Set(current.domain.folds.map((_, i) => i));
  renderReview();
};
$("reviewed").onclick = () => {
  const id = current.meta.id;
  reviewed.has(id) ? reviewed.delete(id) : reviewed.add(id);
  localStorage.setItem("diff-lab-reviewed", JSON.stringify([...reviewed]));
  navigation();
};
$("download").onclick = () => {
  const url = URL.createObjectURL(
    new Blob([JSON.stringify(current.domain, null, 2) + "\n"], {
      type: "application/json",
    }),
  );
  const a = document.createElement("a");
  a.href = url;
  a.download = `${current.meta.id}.domain.json`;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
};
function fail(error) {
  $("status").textContent = error.message;
  console.error(error);
}
try {
  const response = await fetch("data/index.json");
  if (!response.ok)
    throw Error("Run python3 examples/review/viewer/build.py first.");
  fixtures = await response.json();
  await select(
    fixtures.find((f) => f.id === location.hash.slice(1))?.id || fixtures[0].id,
  );
} catch (error) {
  fail(error);
}
