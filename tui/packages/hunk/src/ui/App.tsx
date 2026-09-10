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
import type { DiffFile } from "../diffr/wire";
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
    [wrap, setWrap] = useState(false),
    [isLight, setLight] = useState(false);
  const [scroll, setScroll] = useState(0),
    [horizontal, setHorizontal] = useState(0),
    [closed, setClosed] = useState<Set<number>>(new Set());
  const [selection, setSelection] = useState<SourceSelection | null>(null),
    [message, setMessage] = useState("");
  const dragging = useRef(false),
    thumbDragging = useRef(false);
  const theme = isLight ? light : dark,
    sidebar = width >= 90 ? 28 : 0;
  const contentWidth = Math.max(10, width - sidebar - 1),
    viewportHeight = Math.max(1, height - 2);
  const layout =
    mode === "auto" ? (contentWidth >= 100 ? "split" : "unified") : mode;
  const rowCache = useRef(
    new WeakMap<DiffFile, { key: string; rows: ViewerRow[] }>(),
  );
  const rows = useMemo(() => {
    const all = snapshot.files.flatMap((file, index) => {
      const key = `${index}:${layout}:${isLight}`;
      let cached = rowCache.current.get(file);
      if (cached?.key !== key) {
        cached = { key, rows: rowsForFile(file, index, layout, theme) };
        rowCache.current.set(file, cached);
      }
      return closed.has(index) ? cached.rows.slice(0, 1) : cached.rows;
    });
    for (const [i, error] of snapshot.errors.entries())
      all.push({ key: `error:${i}`, fileIndex: -1, label: error });
    return all;
  }, [snapshot.files, snapshot.errors, layout, theme, closed]);
  const geometry = useMemo(
    () => measureRows(rows, contentWidth, wrap, horizontal),
    [rows, contentWidth, wrap, horizontal],
  );
  const maxScroll = Math.max(0, geometry.height - viewportHeight),
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
    const headers = geometry.rows.filter((r) => r.row.label?.startsWith("@@"));
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
    if (key.name === "q" || (key.ctrl && key.name === "c")) onQuit();
    else if (key.name === "down" || key.name === "j") move(1);
    else if (key.name === "up" || key.name === "k") move(-1);
    else if (key.name === "pagedown" || key.name === "space")
      move(viewportHeight);
    else if (key.name === "pageup") move(-viewportHeight);
    else if (key.name === "home") setScroll(0);
    else if (key.name === "end") setScroll(maxScroll);
    else if (key.name === "right") setHorizontal((n) => n + 4);
    else if (key.name === "left") setHorizontal((n) => Math.max(0, n - 4));
    else if (key.name === "]") navigateHunk(1);
    else if (key.name === "[") navigateHunk(-1);
    else if (key.name === "s") {
      setMode(layout === "split" ? "unified" : "split");
      setSelection(null);
    } else if (key.name === "w") setWrap((v) => !v);
    else if (key.name === "t") setLight((v) => !v);
    else if (key.name === "y") copy();
    else if (key.name === "escape") setSelection(null);
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
  const rendered = [];
  for (const measured of visibleRows(geometry, top, viewportHeight)) {
    const row = measured.row,
      index = indices.get(row.key)!;
    for (
      let line = Math.max(0, top - measured.top);
      line < measured.height && measured.top + line < top + viewportHeight;
      line++
    ) {
      if (row.label !== undefined)
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
  const currentFile = visibleRows(geometry, top, 1)[0]?.row.fileIndex ?? 0;
  const sidebarStart = Math.max(
    0,
    Math.min(
      currentFile - Math.floor(viewportHeight / 2),
      snapshot.files.length - viewportHeight,
    ),
  );
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
  return (
    <box
      width={width}
      height={height}
      flexDirection="column"
      backgroundColor={theme.bg}
      onMouseDrag={(event) => {
        if (thumbDragging.current) scrub(event.y);
        else if (dragging.current) {
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
      }}
    >
      <box height={1} flexDirection="row">
        <text fg={theme.fg} selectable={false}>
          {" "}
          diffr{" "}
        </text>
        <text
          fg={theme.type}
          selectable={false}
          onMouseUp={() => {
            setMode(layout === "split" ? "unified" : "split");
            setSelection(null);
          }}
        >
          {" "}
          {layout} [s]{" "}
        </text>
        <text
          fg={theme.type}
          selectable={false}
          onMouseUp={() => setWrap((v) => !v)}
        >
          {" "}
          wrap {wrap ? "on" : "off"} [w]{" "}
        </text>
        <text
          fg={theme.type}
          selectable={false}
          onMouseUp={() => setLight((v) => !v)}
        >
          {" "}
          theme [t]{" "}
        </text>
        <text fg={theme.type} selectable={false} onMouseUp={copy}>
          {" "}
          copy [y]{" "}
        </text>
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
          <box width={sidebar} flexDirection="column">
            {snapshot.files
              .slice(sidebarStart, sidebarStart + viewportHeight)
              .map((file, i) => (
                <text
                  key={i + sidebarStart}
                  height={1}
                  fg={i + sidebarStart === currentFile ? theme.fg : theme.muted}
                  selectable={false}
                  onMouseUp={() => jump(i + sidebarStart)}
                >
                  {fit(
                    sanitizeTerminalLine(
                      file.file.new_path ?? file.file.old_path ?? "",
                    ),
                    sidebar - 1,
                  )}
                </text>
              ))}
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
            if (thumbDragging.current) scrub(event.y);
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
      <text height={1} fg={theme.muted} selectable={false}>
        {fit(
          `${snapshot.files.length}/${snapshot.total} files ${snapshot.complete ? "" : "loading…"} ${snapshot.errors.length ? `${snapshot.errors.length} errors` : ""}  [/] hunks · drag selects lines · y copy · q quit ${message}`,
          width,
        )}
      </text>
    </box>
  );
}
