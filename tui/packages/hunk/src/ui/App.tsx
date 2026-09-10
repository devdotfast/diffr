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
import {
  dark,
  light,
  rowsForFile,
  type Layout,
  type ViewerRow,
} from "../diffr/rows";
import { measureRows, visibleRows } from "../diffr/geometry";
import {
  copySelection,
  selectionBounds,
  type SourceSelection,
} from "../diffr/selection";
import { fileIdentity, type DiffFile } from "../diffr/wire";
import type { DiffStore } from "../diffr/store";
import { sanitizeTerminalLine } from "../lib/terminalText";
import { sliceTextByWidth } from "./lib/text";
const fit = (text: string, width: number) =>
  sliceTextByWidth(text, 0, width).text;
export function App({
  store,
  onQuit,
}: {
  store: DiffStore;
  onQuit: () => void;
}) {
  const snapshot = useSyncExternalStore(store.subscribe, store.getSnapshot);
  const renderer = useRenderer(),
    { width, height } = useTerminalDimensions();
  const [mode, setMode] = useState<Layout | "auto">("auto"),
    [showSidebar, setShowSidebar] = useState(true),
    [wrap, setWrap] = useState(false),
    [fullContext, setFullContext] = useState(false),
    [isLight, setLight] = useState(false);
  const [scroll, setScroll] = useState(0),
    [horizontal, setHorizontal] = useState(0),
    [closed, setClosed] = useState<Set<number>>(new Set());
  const [selection, setSelection] = useState<SourceSelection | null>(null),
    [message, setMessage] = useState("");
  const [closedDirectories, setClosedDirectories] = useState<Set<string>>(new Set());
  const [treeScroll, setTreeScroll] = useState(0);
  const [sidebarWidth, setSidebarWidth] = useState(28);
  const sidebarDrag = useRef<{ x: number; width: number } | null>(null);
  const [pendingFile, setPendingFile] = useState<string | null>(null);
  const [menu, setMenu] = useState<string | null>(null);
  const dragging = useRef(false),
    thumbDragging = useRef(false);
  const theme = isLight ? light : dark,
    sidebar = showSidebar && width >= 60 ? Math.max(16, Math.min(sidebarWidth, width - 40)) : 0;
  const contentWidth = Math.max(10, width - sidebar - 1),
    viewportHeight = Math.max(1, height - 2);
  const layout =
    mode === "auto" ? (contentWidth >= 100 ? "split" : "unified") : mode;
  const loadedByIdentity = useMemo(() => new Map(snapshot.files.map((f, i) => [fileIdentity(f.file), i])), [snapshot.files]);
  const inventory = useMemo(() => snapshot.inventory.map(file => ({file})), [snapshot.inventory]);
  const tree = useMemo(() => buildFileTree(inventory), [inventory]);
  // Keep loaded indexes stable for row keys, selections and file expansion.
  // Only presentation order changes once the stream is complete.
  const fileOrder = useMemo(() => snapshot.complete
    ? flattenFileTree(tree, new Set()).flatMap(({node}) => {
        if (node.fileIndex === undefined) return [];
        const loaded = loadedByIdentity.get(fileIdentity(snapshot.inventory[node.fileIndex]));
        return loaded === undefined ? [] : [loaded];
      })
    : snapshot.files.map((_, index) => index),
    [snapshot.complete, snapshot.files, snapshot.inventory, tree, loadedByIdentity]);
  const rowCache = useRef(
    new WeakMap<DiffFile, { key: string; rows: ViewerRow[] }>(),
  );
  const rows = useMemo(() => {
    const all = fileOrder.flatMap(index => {
      const file = snapshot.files[index];
      const key = `${index}:${layout}:${isLight}:${fullContext}`;
      let cached = rowCache.current.get(file);
      if (cached?.key !== key) {
        cached = { key, rows: rowsForFile(file, index, layout, theme, fullContext) };
        rowCache.current.set(file, cached);
      }
      return closed.has(index) ? cached.rows.slice(0, 1) : cached.rows;
    });
    for (const [i, error] of snapshot.errors.entries())
      all.push({ key: `error:${i}`, fileIndex: -1, label: error });
    return all;
  }, [snapshot.files, snapshot.errors, layout, theme, closed, fullContext, fileOrder]);
  const geometry = useMemo(
    () => measureRows(rows, contentWidth, wrap, horizontal),
    [rows, contentWidth, wrap, horizontal],
  );
  const lastFileTop = useMemo(() =>
    geometry.rows.findLast(r => r.row.key.endsWith(":header"))?.top ?? 0, [geometry]);
  const maxScroll = Math.max(lastFileTop, geometry.height - viewportHeight),
    top = Math.min(scroll, maxScroll);
  useEffect(() => {
    if (scroll > maxScroll) setScroll(maxScroll);
  }, [scroll, maxScroll]);
  const previousGeometry = useRef(geometry);
  useEffect(() => {
    const previous = previousGeometry.current;
    if (previous !== geometry) {
      const anchor = visibleRows(previous, scroll, 1)[0];
      if (anchor) {
        const next = geometry.rows.find((r) => r.row.key === anchor.row.key);
        if (next)
          setScroll(
            Math.min(
              Math.max(
                0,
                next.top + Math.min(scroll - anchor.top, next.height - 1),
              ),
              maxScroll,
            ),
          );
      }
      previousGeometry.current = geometry;
    }
  }, [geometry, maxScroll]);
  const move = (amount: number) =>
    setScroll((current) => Math.max(0, Math.min(maxScroll, current + amount)));
  const toggleFile = (index: number) => {
    setClosed((old) => {
      const next = new Set(old);
      if (next.has(index)) next.delete(index);
      else next.add(index);
      return next;
    });
  };
  const jump = (index: number) => {
    const row = geometry.rows.find((r) => r.row.fileIndex === index);
    if (row) setScroll(Math.min(maxScroll, row.top));
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
    else if (is("g", "home")) setScroll(0);
    else if (is("G", "end")) setScroll(maxScroll);
    else if (is("right", "shift+right", "l")) setHorizontal(n => n + (key.shift ? 16 : 4));
    else if (is("left", "shift+left", "h")) setHorizontal(n => Math.max(0, n - (key.shift ? 16 : 4)));
    else if (key.name === "]") navigateHunk(1);
    else if (key.name === "[") navigateHunk(-1);
    else if (key.name === "s") {
      setMode(layout === "split" ? "unified" : "split");
      setSelection(null);
    } else if (key.name === "w") setWrap((v) => !v);
    else if (key.name === "c") { setFullContext(v => !v); setSelection(null); }
    else if (key.name === "t") setLight((v) => !v);
    else if (key.name === "y") copy();
    else if (key.name === "escape") { setSelection(null); setMenu(null); }
    else if (key.name === "return") {
      const current = visibleRows(geometry, top, 1)[0];
      if (current && current.row.fileIndex >= 0)
        toggleFile(current.row.fileIndex);
    }
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
  const currentTreeFile = snapshot.inventory.findIndex(file => fileIdentity(file) === activeIdentity);
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
  const treeRows = useMemo(() => {
    if (snapshot.complete) return flattenFileTree(tree, closedDirectories);
    const manifestIndexes = new Map(snapshot.inventory.map((file, index) => [fileIdentity(file), index]));
    const ready = snapshot.files.map(file => manifestIndexes.get(fileIdentity(file.file))!);
    const shown = new Set(ready);
    const order = [...ready, ...snapshot.inventory.map((_, i) => i).filter(i => !shown.has(i))];
    return order.map(fileIndex => ({
      depth: 0,
      node: {key: `file:${fileIndex}`, fileIndex, children: [],
        name: snapshot.inventory[fileIndex].new_path ?? snapshot.inventory[fileIndex].old_path ?? ""},
    }));
  }, [snapshot.complete, snapshot.files, snapshot.inventory, tree, closedDirectories]);
  const counts = useMemo(() => snapshot.files.map(lineCounts), [snapshot.files]);
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
    const file = snapshot.files[fileIndex], count = counts[fileIndex];
    if (!file) return null;
    const path = file.file.new_path ?? file.file.old_path ?? file.diff.display_path;
    const statsWidth = String(count.added).length + String(count.removed).length + 5;
    return <box key={key} height={1} width={contentWidth} flexDirection="row"
      backgroundColor={isLight ? "#f0f2f5" : "#161b22"}
      onMouseUp={() => toggleFile(fileIndex)}>
      <text width={Math.max(1, contentWidth - statsWidth)} fg={theme.fg} selectable={false}>
        {fit(sanitizeTerminalLine(`${closed.has(fileIndex) ? "▸" : "▾"} ${path}`), Math.max(1, contentWidth - statsWidth))}
      </text>
      <text fg={isLight ? "#1a7f37" : "#7ee787"} selectable={false}>{` +${count.added}`}</text>
      <text fg={isLight ? "#cf222e" : "#ffa198"} selectable={false}>{` -${count.removed} `}</text>
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
            fg={theme.muted}
            selectable={false}
            onMouseUp={() => {
              if (row.key.endsWith(":header")) toggleFile(row.fileIndex);
            }}
          >
            {fit(
              sanitizeTerminalLine(
                row.key.endsWith(":header")
                  ? `${closed.has(row.fileIndex) ? "▸" : "▾"} ${row.label}`
                  : row.label,
              ),
              contentWidth,
            )}
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
      [`Context: ${fullContext ? "all" : "compact"}  c`, () => { setFullContext(v => !v); setSelection(null); }]],
    Navigate: [["Previous change  [", () => navigateHunk(-1)], ["Next change  ]", () => navigateHunk(1)],
      ["First file  Home", () => setScroll(0)], ["Last file  End", () => setScroll(maxScroll)]],
    Theme: [["Dark", () => setLight(false)], ["Light", () => setLight(true)]],
    Help: [["Scroll: j/k · h/l · gg/G", () => setMessage("j/k scroll · h/l pan · gg first · G last")],
      ["Half page: Ctrl-D / Ctrl-U", () => setMessage("d / Ctrl-D: half down · u / Ctrl-U: half up")],
      ["Full page: Ctrl-F / Ctrl-B", () => setMessage("Ctrl-F: page down · Ctrl-B: page up")],
      ["Drag to select · y to copy", () => setMessage("Drag source lines; y copies original source")]],
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
      <box height={1} flexDirection="row" backgroundColor={isLight ? "#f0f2f5" : "#161b22"}>
        {["File", "View", "Navigate", "Theme", "Help"].map(name => (
          <text key={name} fg={menu === name ? theme.fg : theme.muted} selectable={false}
            onMouseUp={() => setMenu(old => old === name ? null : name)}>
            {` ${name} `}
          </text>
        ))}
        <text fg={theme.type} selectable={false} onMouseUp={() => {
          setMode(layout === "split" ? "unified" : "split"); setSelection(null);
        }}>{`  ${layout} [s] `}</text>
        <text fg={theme.muted} selectable={false}>{` · ${snapshot.total || snapshot.inventory.length} files`}</text>
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
                bg={node.fileIndex === currentTreeFile ? (isLight ? "#ddf4ff" : "#1c3045") : theme.bg}
                selectable={false}
                onMouseUp={() => {
                  if (node.fileIndex !== undefined) {
                    const identity = fileIdentity(snapshot.inventory[node.fileIndex]);
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
                  ? (closedDirectories.has(node.key) ? "▸ " : "▾ ") : snapshot.failedFiles.has(fileIdentity(snapshot.inventory[node.fileIndex])) ? "! "
                    : loadedByIdentity.has(fileIdentity(snapshot.inventory[node.fileIndex])) ? "  " : "◌ ") + node.name), sidebar - 1)}
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
      {menu && <box position="absolute" top={1} left={0} width={38}
        height={menuItems[menu].length} flexDirection="column" zIndex={10}
        backgroundColor={isLight ? "#eaeef2" : "#21262d"}>
        {menuItems[menu].map(([label, action]) => (
          <text key={label} height={1} width={38} fg={theme.fg} selectable={false}
            onMouseUp={() => { action(); setMenu(null); }}>
            {" " + label}
          </text>
        ))}
      </box>}
      <text height={1} fg={theme.muted} selectable={false}>
        {fit(
          `${snapshot.files.length}/${snapshot.total} files ${snapshot.complete ? "" : "loading…"} ${snapshot.errors.length ? `${snapshot.errors.length} errors` : ""}  [/] hunks · drag selects lines · y copy · q quit ${message}`,
          width,
        )}
      </text>
    </box>
  );
}
