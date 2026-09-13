/** Coordinate viewer interactions; Rust owns every comparison and source correspondence. */
import {
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
} from "react";
import {
  useKeyboard,
  useRenderer,
  useTerminalDimensions,
} from "@opentui/react";
import { buildFileTree, flattenFileTree, parentDirectories, lineCounts } from "../diffr/fileTree";
import { matchesKey } from "./lib/keys";
import { resizeSidebarWidth } from "./lib/sidebar";
import { CodeRowView } from "./diff/CodeRowView";
import type { MoveJump } from "./diff/diffRowModel";
import {
  rowsForFile,
  type Layout,
  type ViewerRow,
} from "../diffr/rows";
import type { Palette, ThemeSet } from "../diffr/theme";
import { measureRows, visibleRows } from "../diffr/geometry";
import {
  copySelection,
  selectionBounds,
  type SourceSelection,
} from "../diffr/selection";
import { fileIdentity, filePath, type DiffFile } from "../diffr/wire";
import { defaultCollapsed, foldIds, gapIds, nestedIds, type RowFold } from "../diffr/regions";
import { placeholderRows } from "../diffr/rows";
import { add, blockBar, comparisonLabel, zero, type LineCounts } from "../diffr/counts";
import type { DiffStore } from "../diffr/store";
import { sanitizeTerminalLine } from "../lib/terminalText";
import { sliceTextByWidth } from "./lib/text";
const fit = (text: string, width: number) =>
  sliceTextByWidth(text, 0, width).text;
export function App({
  store,
  onQuit,
  themes,
}: {
  store: DiffStore;
  onQuit: () => void;
  themes: ThemeSet;
}) {
  const snapshot = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const renderer = useRenderer(),
    { width, height } = useTerminalDimensions();
  const [mode, setMode] = useState<Layout | "auto">("auto"),
    [showSidebar, setShowSidebar] = useState(true),
    [wrap, setWrap] = useState(false),
    [theme, setTheme] = useState<Palette>(themes.initial);
  const [scroll, setScroll] = useState(0),
    [horizontal, setHorizontal] = useState(0);
  // Files the user closed or opened; unset files follow the manifest's visibility.
  const [closed, setClosed] = useState<Map<number, boolean>>(new Map());
  const [selection, setSelection] = useState<SourceSelection | null>(null),
    [message, setMessage] = useState("");
  // Fold ids collapsed per loaded file; unset files start where diffr's visibility says.
  const [collapsed, setCollapsed] = useState<Map<number, ReadonlySet<number>>>(new Map());
  // Vim's z prefix: the next key names the fold command.
  const pendingZ = useRef(false);
  // `g` goes home at once but remembers where the view was, so a following `m` can jump from there
  // to the other copy of moved code.
  const pendingG = useRef<number | null>(null);
  const [closedDirectories, setClosedDirectories] = useState<Set<string>>(new Set());
  const [treeScroll, setTreeScroll] = useState(0);
  const [sidebarWidth, setSidebarWidth] = useState(28);
  const sidebarDrag = useRef<{ x: number; width: number } | null>(null);
  const [pendingFile, setPendingFile] = useState<string | null>(null);
  const [menu, setMenu] = useState<string | null>(null);
  const [showBreakdown, setShowBreakdown] = useState(false);
  const dragging = useRef(false),
    thumbDragging = useRef(false);
  // `t` swaps between the two bundled defaults; a configured theme is left by the first press.
  const toggleTheme = () => setTheme((current) => (current.isLight ? themes.dark : themes.light));
  const sidebar = showSidebar && width >= 60 ? Math.max(16, Math.min(sidebarWidth, width - 40)) : 0;
  const contentWidth = Math.max(10, width - sidebar - 1),
    viewportHeight = Math.max(1, height - 3);
  const layout =
    mode === "auto" ? (contentWidth >= 100 ? "split" : "unified") : mode;
  const loadedByIdentity = useMemo(() => new Map(snapshot.files.map((f, i) => [fileIdentity(f.file), i])), [snapshot.files]);
  const inventory = snapshot.inventory;
  const manifestOf = (file: DiffFile) => inventory.find(entry => fileIdentity(entry.file) === fileIdentity(file.file));
  const foldsOf = (index: number): ReadonlySet<number> => {
    const file = snapshot.files[index];
    return collapsed.get(index) ?? (file.diff.type === "text" ? defaultCollapsed(file.diff) : new Set());
  };
  const isClosed = (index: number) => closed.get(index) ?? manifestOf(snapshot.files[index])?.visibility.collapsed ?? false;
  const tree = useMemo(() => buildFileTree(inventory), [inventory]);
  // Keep loaded indexes stable for row keys, selections and file expansion.
  // Present arriving diffs in tree order throughout loading.
  const fileOrder = useMemo(() => flattenFileTree(tree, new Set()).flatMap(({node}) => {
        if (node.fileIndex === undefined) return [];
        const loaded = loadedByIdentity.get(fileIdentity(snapshot.inventory[node.fileIndex].file));
        return loaded === undefined ? [] : [loaded];
      }),
    [snapshot.inventory, tree, loadedByIdentity]);
  const rowCache = useRef(
    new WeakMap<DiffFile, { key: string; rows: ViewerRow[] }>(),
  );
  const rows = useMemo(() => {
    const all = fileOrder.flatMap(index => {
      const file = snapshot.files[index];
      const folds = foldsOf(index);
      const key = `${index}:${layout}:${theme.name}:${[...folds].sort((a, b) => a - b).join(",")}`;
      let cached = rowCache.current.get(file);
      if (cached?.key !== key) {
        cached = { key, rows: rowsForFile(file, index, layout, theme, folds) };
        rowCache.current.set(file, cached);
      }
      if (!isClosed(index)) return cached.rows;
      const manifest = manifestOf(file);
      return manifest?.visibility.collapsed
        ? [cached.rows[0], ...placeholderRows(index, manifest.visibility.label)]
        : cached.rows.slice(0, 1);
    });
    for (const [i, error] of snapshot.errors.entries())
      all.push({ key: `error:${i}`, fileIndex: -1, label: error });
    return all;
  }, [snapshot.files, snapshot.errors, snapshot.inventory, layout, theme, closed, fileOrder, collapsed]);
  const geometry = useMemo(
    () => measureRows(rows, contentWidth, wrap, horizontal),
    [rows, contentWidth, wrap, horizontal],
  );
  const lastFileTop = useMemo(() =>
    geometry.rows.findLast(r => r.row.key.endsWith(":header"))?.top ?? 0, [geometry]);
  const maxScroll = Math.max(lastFileTop, geometry.height - viewportHeight),
    top = Math.min(scroll, maxScroll);
  // Reconcile the offset before committing new geometry. An effect would first
  // mount the new rows at the old offset, exposing a wrong viewport for one frame.
  const [previousGeometry, setPreviousGeometry] = useState(geometry);
  if (previousGeometry !== geometry) {
    const anchor = visibleRows(previousGeometry, scroll, 1)[0];
    const next = anchor && geometry.rows.find(r => r.row.key === anchor.row.key);
    const nextScroll = next && anchor
      ? next.top + Math.min(scroll - anchor.top, next.height - 1)
      : scroll;
    setPreviousGeometry(geometry);
    setScroll(Math.max(0, Math.min(nextScroll, maxScroll)));
  } else if (scroll > maxScroll) {
    setScroll(maxScroll);
  }
  const move = (amount: number) =>
    setScroll((current) => Math.max(0, Math.min(maxScroll, current + amount)));
  const toggleFile = (index: number) =>
    setClosed((old) => new Map(old).set(index, !isClosed(index)));
  const setFolds = (fileIndex: number, ids: number[], collapse: boolean) =>
    setCollapsed((old) => {
      const next = new Set(foldsOf(fileIndex));
      for (const id of ids) if (collapse) next.add(id); else next.delete(id);
      return new Map(old).set(fileIndex, next);
    });
  // Recursive commands (Alt-click, zC, zO, zA) include every fold nested inside.
  const setFold = (fileIndex: number, fold: RowFold, collapse: boolean, recursive: boolean) => {
    const file = snapshot.files[fileIndex];
    if (file.diff.type !== "text") throw new Error("Binary files have no folds");
    const ids = recursive ? [fold.id, ...nestedIds(file.diff, fold.id)] : [fold.id];
    setFolds(fileIndex, ids, collapse);
  };
  // `c`: reveal every context gap, or hide them again.
  const toggleContext = () => {
    const opened = snapshot.files.some((file, index) =>
      file.diff.type === "text" && gapIds(file.diff).some((id) => !foldsOf(index).has(id)));
    snapshot.files.forEach((file, index) => {
      if (file.diff.type === "text") setFolds(index, gapIds(file.diff), opened);
    });
  };
  const toggleFold = (fileIndex: number, fold: RowFold, recursive: boolean) =>
    setFold(fileIndex, fold, !fold.collapsed, recursive);
  const rowFold = (row: ViewerRow) => row.cell?.fold ?? row.right?.fold ?? row.left?.fold;
  const navigateFold = (direction: number) => {
    const headers = geometry.rows.filter((r) => rowFold(r.row));
    const target =
      direction > 0
        ? headers.find((r) => r.top > top)
        : headers.findLast((r) => r.top < top);
    if (target) setScroll(Math.min(maxScroll, target.top));
  };
  // Vim fold commands act on the fold whose header is the top visible row.
  const foldCommand = (command: string) => {
    if (command === "R") return foldAll(false);
    if (command === "M") return foldAll(true);
    if (command === "j") return navigateFold(1);
    if (command === "k") return navigateFold(-1);
    const current = visibleRows(geometry, top, 1)[0]?.row;
    const fold = current && rowFold(current);
    if (!current || !fold) return;
    const recursive = command === command.toUpperCase();
    const letter = command.toLowerCase();
    if (letter === "a") toggleFold(current.fileIndex, fold, recursive);
    else if (letter === "o") setFold(current.fileIndex, fold, false, recursive);
    else if (letter === "c") setFold(current.fileIndex, fold, true, recursive);
  };
  const foldAll = (collapse: boolean) =>
    snapshot.files.forEach((file, index) => {
      if (file.diff.type === "text") setFolds(index, foldIds(file.diff), collapse);
    });
  const jump = (index: number) => {
    const row = geometry.rows.find((r) => r.row.fileIndex === index);
    if (row) setScroll(Math.min(maxScroll, row.top));
  };
  // Moved code: scroll to the counterpart line on the other side, in the same file.
  const jumpToMove = (fileIndex: number, target: MoveJump) => {
    const lineOf = (row: ViewerRow) => target.side === "left"
      ? (row.left?.lineNumber ?? row.cell?.oldLineNumber)
      : (row.right?.lineNumber ?? row.cell?.newLineNumber);
    const found = geometry.rows.find((r) => r.row.fileIndex === fileIndex && lineOf(r.row) === target.line);
    if (!found) return setMessage(`Line ${target.line} is folded or not loaded`);
    setScroll(Math.min(maxScroll, found.top));
    setMessage(`Moved code: ${target.side} line ${target.line}`);
  };
  const jumpFromTop = (from = top) => {
    const current = visibleRows(geometry, from, Math.max(1, viewportHeight)).map((r) => r.row)
      .find((row) => (row.cell ?? row.right)?.jump ?? row.left?.jump);
    const jumpOf = current && ((current.cell ?? current.right)?.jump ?? current.left?.jump);
    if (!current || !jumpOf) return setMessage("No moved code on screen");
    jumpToMove(current.fileIndex, jumpOf);
  };
  const navigateHunk = (direction: number) => {
    const headers = geometry.rows.filter((r) => r.row.hunkStart);
    const target =
      direction > 0
        ? headers.find((r) => r.top > top)
        : headers.findLast((r) => r.top < top);
    if (target) setScroll(Math.min(maxScroll, target.top));
  };
  const copy = () => {
    if (!selection) return;
    const text = copySelection(snapshot.files, rows, selection);
    if (text) {
      renderer.copyToClipboardOSC52(text);
      setMessage("Copied source lines");
    }
  };
  useKeyboard((key) => {
    // Hunk's chord matcher handles raw control bytes and Kitty events alike.
    const is = (...chords: string[]) => !key.super && chords.some(chord => matchesKey(chord, key));
    if (pendingG.current !== null) {
      const from = pendingG.current;
      pendingG.current = null;
      if (is("m")) return jumpFromTop(from);
    }
    if (pendingZ.current) {
      pendingZ.current = false;
      const command = key.shift ? key.name?.toUpperCase() : key.name;
      if (!key.ctrl && !key.meta && command?.length === 1 && "aocAOCRMjk".includes(command))
        foldCommand(command);
      return;
    }
    if (is("q", "ctrl+c")) onQuit();
    else if ((key.name === "b" && (key.super || key.meta)) || is("\\")) {
      key.preventDefault(); setShowSidebar(v => !v);
    }
    else if (is("d", "ctrl+d", "u", "ctrl+u"))
      move((is("d", "ctrl+d") ? 1 : -1) * Math.max(1, Math.floor(viewportHeight / 2)));
    else if (is("pagedown", "space", "f", "ctrl+f")) move(viewportHeight);
    else if (is("pageup", "b", "shift+space", "ctrl+b")) move(-viewportHeight);
    else if (is("down", "j")) move(1);
    else if (is("up", "k")) move(-1);
    else if (is("g")) { pendingG.current = top; setScroll(0); }
    else if (is("home")) setScroll(0);
    else if (is("G", "end")) setScroll(maxScroll);
    else if (is("right", "shift+right", "l")) setHorizontal(n => n + (key.shift ? 16 : 4));
    else if (is("left", "shift+left", "h")) setHorizontal(n => Math.max(0, n - (key.shift ? 16 : 4)));
    else if (key.name === "]") navigateHunk(1);
    else if (key.name === "[") navigateHunk(-1);
    else if (key.name === "s") {
      setMode(layout === "split" ? "unified" : "split");
      setSelection(null);
    } else if (key.name === "w") setWrap((v) => !v);
    else if (key.name === "c") { toggleContext(); setSelection(null); }
    else if (key.name === "t") toggleTheme();
    else if (key.name === "y") copy();
    else if (key.name === "escape") { setSelection(null); setMenu(null); setShowBreakdown(false); }
    else if (key.name === "i") setShowBreakdown((v) => !v);
    else if (key.name === "return") {
      const current = visibleRows(geometry, top, 1)[0];
      if (current && current.row.fileIndex >= 0)
        toggleFile(current.row.fileIndex);
    } else if (is("z")) pendingZ.current = true;
  });
  const [selectionStart, selectionEnd] = useMemo(
    () => selectionBounds(rows, selection),
    [rows, selection],
  );
  const indices = useMemo(
    () => new Map(rows.map((row, i) => [row.key, i])),
    [rows],
  );
  const viewport = visibleRows(geometry, top, viewportHeight);
  const currentFile = viewport[0]?.row.fileIndex ?? 0;
  const activeIdentity = pendingFile ?? (snapshot.files[currentFile] ? fileIdentity(snapshot.files[currentFile].file) : null);
  const currentTreeFile = snapshot.inventory.findIndex(entry => fileIdentity(entry.file) === activeIdentity);
  useEffect(() => {
    if (pendingFile === null) return;
    const index = loadedByIdentity.get(pendingFile);
    if (index !== undefined) {
      const row = geometry.rows.find(r => r.row.fileIndex === index);
      if (row) { setScroll(Math.min(maxScroll, row.top)); setPendingFile(null); setMessage(""); }
    } else if (snapshot.failedFiles.has(pendingFile) || snapshot.complete) {
      setMessage(snapshot.failedFiles.get(pendingFile) ?? "File did not load");
      setPendingFile(null);
    }
  }, [pendingFile, loadedByIdentity, geometry, maxScroll, snapshot.failedFiles, snapshot.complete]);
  const treeRows = useMemo(() => flattenFileTree(tree, closedDirectories), [tree, closedDirectories]);
  const counts = useMemo(() => snapshot.files.map(lineCounts), [snapshot.files]);
  // Headline numbers are diffr's stats.visible, verbatim: folding never changes them.
  const totals = useMemo(() => ({
    visible: counts.reduce((sum, c) => add(sum, c.visible), zero),
    textual: counts.reduce((sum, c) => add(sum, c.textual), zero),
    fallbacks: counts.filter((c) => c.fallback).length,
  }), [counts]);
  const plusMinus = (c: LineCounts) => `+${c.added} −${c.removed}`;
  useEffect(() => {
    const file = inventory[currentTreeFile];
    if (!file) return;
    setClosedDirectories(old => {
      const next = new Set(old);
      parentDirectories(file).forEach(path => next.delete(path));
      return next.size === old.size ? old : next;
    });
  }, [currentTreeFile, inventory]);
  useEffect(() => {
    const index = treeRows.findIndex(r => r.node.fileIndex === currentTreeFile);
    if (index >= 0) setTreeScroll(old => index < old ? index
      : index >= old + viewportHeight ? index - viewportHeight + 1 : old);
  }, [currentTreeFile, treeRows, viewportHeight]);
  const sidebarStart = Math.min(treeScroll, Math.max(0, treeRows.length - viewportHeight));
  const fileHeader = (fileIndex: number, key: string) => {
    const file = snapshot.files[fileIndex], count = counts[fileIndex]?.visible;
    if (!file || !count) return null;
    const path = filePath(file.file);
    const statsWidth = String(count.added).length + String(count.removed).length + 5;
    return <box key={key} height={1} width={contentWidth} flexDirection="row"
      backgroundColor={theme.chrome}
      onMouseUp={() => toggleFile(fileIndex)}>
      <text width={Math.max(1, contentWidth - statsWidth)} fg={theme.fg} selectable={false}>
        {fit(sanitizeTerminalLine(`${isClosed(fileIndex) ? "▸" : "▾"} ${path}`), Math.max(1, contentWidth - statsWidth))}
      </text>
      <text fg={theme.addedText} selectable={false}>{` +${count.added}`}</text>
      <text fg={theme.removedText} selectable={false}>{` −${count.removed} `}</text>
    </box>;
  };
  const rendered = [];

  for (const measured of viewport) {
    const row = measured.row,
      index = indices.get(row.key)!;
    for (
      let line = Math.max(0, top - measured.top);
      line < measured.height && measured.top + line < top + viewportHeight;
      line++
    ) {
      if (row.key.endsWith(":header"))
        rendered.push(fileHeader(row.fileIndex, row.key));
      else if (row.label !== undefined)
        rendered.push(
          <text
            key={row.key}
            height={1}
            width={contentWidth}
            fg={row.loadDiff ? theme.accent : theme.muted}
            selectable={false}
            onMouseUp={() => {
              if (row.loadDiff) toggleFile(row.fileIndex);
            }}
          >
            {fit(sanitizeTerminalLine(row.loadDiff ? `    ${row.label}` : row.label), contentWidth)}
          </text>,
        );
      else
        rendered.push(
          <CodeRowView
            key={`${row.key}:${line}`}
            measured={measured}
            visualLine={line}
            geometry={geometry}
            theme={theme}
            selectedSide={
              selection && index >= selectionStart && index <= selectionEnd
                ? selection.side
                : undefined
            }
            onSelect={(side) => {
              dragging.current = true;
              setSelection({ anchor: row.key, end: row.key, side });
            }}
            onExtend={() => {
              if (dragging.current)
                setSelection((s) => (s ? { ...s, end: row.key } : s));
            }}
            onFold={(fold, recursive) => toggleFold(row.fileIndex, fold, recursive)}
            onJump={(target) => jumpToMove(row.fileIndex, target)}
          />,
        );
    }
  }
  // Overlay the active file header while its original header is above the viewport.
  if (currentFile >= 0 && viewport.length && !viewport[0].row.key.endsWith(":header")) {
    rendered[0] = fileHeader(currentFile, "sticky-header");
  }
  const thumbHeight = Math.max(
    1,
    Math.floor(
      (viewportHeight * viewportHeight) /
        Math.max(viewportHeight, geometry.height),
    ),
  );
  const thumbTop = maxScroll
    ? Math.round((top / maxScroll) * (viewportHeight - thumbHeight))
    : 0;
  function scrub(y: number) {
    setScroll(
      Math.round(
        Math.max(0, Math.min(1, (y - 1) / Math.max(1, viewportHeight - 1))) *
          maxScroll,
      ),
    );
  }
  const menuItems: Record<string, [string, () => void][]> = {
    File: [["Toggle file tree  ⌘B / \\", () => setShowSidebar(v => !v)], ["Copy selection  y", copy], ["Quit  q", onQuit]],
    View: [[`Layout: ${layout}  s`, () => { setMode(layout === "split" ? "unified" : "split"); setSelection(null); }],
      [`Wrap: ${wrap ? "on" : "off"}  w`, () => setWrap(v => !v)],
      ["Toggle context gaps  c", () => { toggleContext(); setSelection(null); }],
      ["Fold all  zM", () => foldAll(true)], ["Unfold all  zR", () => foldAll(false)]],
    Navigate: [["Previous change  [", () => navigateHunk(-1)], ["Next change  ]", () => navigateHunk(1)],
      ["First file  Home", () => setScroll(0)], ["Last file  End", () => setScroll(maxScroll)]],
    Theme: [[`Dark (${themes.dark.name})  t`, () => setTheme(themes.dark)], [`Light (${themes.light.name})  t`, () => setTheme(themes.light)]],
    Help: [["Scroll: j/k · h/l · gg/G", () => setMessage("j/k scroll · h/l pan · gg first · G last")],
      ["Half page: Ctrl-D / Ctrl-U", () => setMessage("d / Ctrl-D: half down · u / Ctrl-U: half up")],
      ["Full page: Ctrl-F / Ctrl-B", () => setMessage("Ctrl-F: page down · Ctrl-B: page up")],
      ["Drag to select · y to copy", () => setMessage("Drag source lines; y copies original source")],
      ["Change breakdown  i", () => setShowBreakdown(true)],
      ["Folds: click ▾ · za zo zc · zM zR", () => setMessage("Click the chevron or ⋯ · za toggle, zo open, zc close the top fold (zA zO zC recursive) · zM/zR fold/unfold all · zj/zk next/previous fold")]],
  };
  return (
    <box
      width={width}
      height={height}
      flexDirection="column"
      backgroundColor={theme.bg}
      onMouseDrag={(event) => {
        if (sidebarDrag.current) {
          setSidebarWidth(resizeSidebarWidth(sidebarDrag.current.width,
            sidebarDrag.current.x, event.x, 16, width - 40));
        } else if (thumbDragging.current) scrub(event.y);
        else if (dragging.current && !(event.y === 1 && !viewport[0]?.row.key.endsWith(":header"))) {
          const target = visibleRows(
            geometry,
            Math.max(0, top + event.y - 1),
            1,
          )[0]?.row;
          if (target && target.label === undefined)
            setSelection((s) => (s ? { ...s, end: target.key } : s));
        }
      }}
      onMouseUp={() => {
        dragging.current = false;
        thumbDragging.current = false;
        sidebarDrag.current = null;
      }}
    >
      <box height={1} flexDirection="row" backgroundColor={theme.chrome}>
        {["File", "View", "Navigate", "Theme", "Help"].map(name => (
          <text key={name} fg={menu === name ? theme.fg : theme.muted} selectable={false}
            onMouseUp={() => setMenu(old => old === name ? null : name)}>
            {` ${name} `}
          </text>
        ))}
        <text fg={theme.accent} selectable={false} onMouseUp={() => {
          setMode(layout === "split" ? "unified" : "split"); setSelection(null);
        }}>{`  ${layout} [s] `}</text>
      </box>
      <box height={1} flexDirection="row" backgroundColor={theme.chrome}>
        <text fg={theme.fg} selectable={false}>
          {` ${snapshot.comparison ? comparisonLabel(snapshot.comparison.lhs, snapshot.comparison.rhs) : "diffr"}`}
        </text>
        <text fg={theme.muted} selectable={false}>{` · ${snapshot.total || snapshot.inventory.length} files · `}</text>
        <text fg={theme.addedText} selectable={false} onMouseUp={() => setShowBreakdown((v) => !v)}>{`+${totals.visible.added}`}</text>
        <text fg={theme.removedText} selectable={false} onMouseUp={() => setShowBreakdown((v) => !v)}>{` −${totals.visible.removed}`}</text>
        <text fg={theme.muted} selectable={false}>{snapshot.complete ? " " : "… "}</text>
        {blockBar(totals.visible).map((block, i) => (
          <text key={i} fg={block === "added" ? theme.addedText : block === "removed" ? theme.removedText : theme.muted}
            selectable={false} onMouseUp={() => setShowBreakdown((v) => !v)}>{block === "neutral" ? "□" : "■"}</text>
        ))}
      </box>
      <box
        height={viewportHeight}
        flexDirection="row"
        onMouseScroll={(event) => {
          const d = event.scroll;
          if (d) {
            if (d.direction === "left" || d.direction === "right")
              setHorizontal((n) =>
                Math.max(0, n + (d.direction === "left" ? -4 : 4)),
              );
            else
              move((d.direction === "up" ? -1 : 1) * Math.max(1, d.delta) * 3);
          }
        }}
      >
        {sidebar > 0 && (
          <box width={sidebar} flexDirection="row">
          <box width={sidebar - 1} flexDirection="column" onMouseScroll={event => {
            event.stopPropagation();
            const scrollEvent = event.scroll;
            if (scrollEvent) setTreeScroll(n => Math.max(0, Math.min(
              Math.max(0, treeRows.length - viewportHeight),
              n + (scrollEvent.direction === "up" ? -3 : 3))));
          }}>
            {treeRows.slice(sidebarStart, sidebarStart + viewportHeight).map(({node, depth}) => (
              <text key={node.key} height={1} width={sidebar - 1}
                fg={node.fileIndex === currentTreeFile ? theme.fg : theme.muted}
                bg={node.fileIndex === currentTreeFile ? theme.highlight : theme.bg}
                selectable={false}
                onMouseUp={() => {
                  if (node.fileIndex !== undefined) {
                    const identity = fileIdentity(snapshot.inventory[node.fileIndex].file);
                    const loaded = loadedByIdentity.get(identity);
                    if (loaded !== undefined) { setPendingFile(null); setMessage(""); jump(loaded); }
                    else if (snapshot.failedFiles.has(identity)) setMessage(snapshot.failedFiles.get(identity)!);
                    else { setPendingFile(identity); setMessage("Waiting for " + node.name + "…"); }
                  }
                  else setClosedDirectories(old => {
                    const next = new Set(old);
                    if (next.has(node.key)) next.delete(node.key); else next.add(node.key);
                    return next;
                  });
                }}>
                {fit(sanitizeTerminalLine("  ".repeat(depth) + (node.fileIndex === undefined
                  ? (closedDirectories.has(node.key) ? "▸ " : "▾ ") : snapshot.failedFiles.has(fileIdentity(snapshot.inventory[node.fileIndex].file)) ? "! "
                    : loadedByIdentity.has(fileIdentity(snapshot.inventory[node.fileIndex].file)) ? "  " : "◌ ") + node.name), sidebar - 1)}
              </text>
            ))}
          </box>
          <box width={1} height={viewportHeight}
            onMouseDown={event => {
              if (event.button !== 0) return;
              event.stopPropagation();
              dragging.current = false;
              sidebarDrag.current = {x: event.x, width: sidebar};
            }}>
            <text width={1} height={viewportHeight} fg={theme.muted} selectable={false}>
              {Array.from({length: viewportHeight}, () => "│").join("\n")}
            </text>
          </box>
          </box>
        )}
        <box
          width={contentWidth}
          height={viewportHeight}
          flexDirection="column"
          overflow="hidden"
        >
          {rendered.length ? (
            rendered
          ) : (
            <text fg={theme.muted}>
              {snapshot.complete ? "No changed files" : "Loading diffr…"}
            </text>
          )}
        </box>
        <box
          width={1}
          height={viewportHeight}
          onMouseDown={(event) => {
            thumbDragging.current = true;
            scrub(event.y);
          }}
          onMouseMove={(event) => {
            if (sidebarDrag.current) {
          setSidebarWidth(resizeSidebarWidth(sidebarDrag.current.width,
            sidebarDrag.current.x, event.x, 16, width - 40));
        } else if (thumbDragging.current) scrub(event.y);
          }}
        >
          <box
            position="absolute"
            top={thumbTop}
            width={1}
            height={thumbHeight}
            backgroundColor={theme.muted}
          />
        </box>
      </box>
      {showBreakdown && (() => {
        const file = snapshot.files[currentFile];
        const fallback = file && counts[currentFile].fallback;
        const sections: [string, LineCounts, LineCounts, string | null][] = [
          ["All files", totals.visible, totals.textual, totals.fallbacks ? `line diff: ${totals.fallbacks} files` : null],
          ...(file ? [[filePath(file.file), counts[currentFile].visible, counts[currentFile].textual,
            fallback ? `line diff: ${fallback.code}` : null] as [string, LineCounts, LineCounts, string | null]] : []),
        ];
        const boxWidth = Math.min(width, 44);
        const rowCount = sections.reduce((n, s) => n + 3 + (s[3] ? 1 : 0), 0);
        return <box position="absolute" top={2} left={Math.max(0, width - boxWidth - 1)} width={boxWidth}
          height={rowCount + 1} flexDirection="column" zIndex={10} backgroundColor={theme.chrome}>
          {sections.flatMap(([title, visible, textual, note]) => [
            <text key={`${title}:t`} height={1} fg={theme.fg} selectable={false}>{fit(` ${title}`, boxWidth)}</text>,
            <text key={`${title}:v`} height={1} fg={theme.muted} selectable={false}>{fit(`   visible   ${plusMinus(visible)}`, boxWidth)}</text>,
            <text key={`${title}:x`} height={1} fg={theme.muted} selectable={false}>{fit(`   textual   ${plusMinus(textual)}`, boxWidth)}</text>,
            ...(note ? [<text key={`${title}:f`} height={1} fg={theme.muted} selectable={false}>{fit(`   ${note}`, boxWidth)}</text>] : []),
          ])}
          <text height={1} fg={theme.muted} selectable={false}>{fit(" esc close", boxWidth)}</text>
        </box>;
      })()}
      {menu && <box position="absolute" top={1} left={0} width={38}
        height={menuItems[menu].length} flexDirection="column" zIndex={10}
        backgroundColor={theme.chrome}>
        {menuItems[menu].map(([label, action]) => (
          <text key={label} height={1} width={38} fg={theme.fg} selectable={false}
            onMouseUp={() => { action(); setMenu(null); }}>
            {" " + label}
          </text>
        ))}
      </box>}
      <text height={1} fg={theme.muted} selectable={false}>
        {fit(
          `${snapshot.files.length}/${snapshot.total} files ${snapshot.complete ? "" : "loading…"} ${snapshot.errors.length ? `${snapshot.errors.length} errors` : ""}  [/] hunks · za fold · i breakdown · drag selects lines · y copy · q quit ${message}`,
          width,
        )}
      </text>
    </box>
  );
}
