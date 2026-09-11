/** A searchable settings screen over diffr's config schema; every change writes through the CLI. */
import { useMemo, useState } from "react";
import { useKeyboard, useTerminalDimensions } from "@opentui/react";
import {
  filterSettings,
  flattenSchema,
  formatValue,
  isDefault,
  parseValue,
  type ConfigClient,
  type Setting,
} from "../diffr/config";
import { dark } from "../diffr/rows";
import { sliceTextByWidth } from "./lib/text";
const fit = (text: string, width: number) => sliceTextByWidth(text, 0, width).text;
const pad = (text: string, width: number) => fit(text, width).padEnd(width);
export function Settings({ client, onQuit, initialQuery = "" }: { client: ConfigClient; onQuit: () => void; initialQuery?: string }) {
  const { width, height } = useTerminalDimensions();
  const [settings, setSettings] = useState<Setting[]>(() => flattenSchema(client.schema(), client.show()));
  const [query, setQuery] = useState(initialQuery),
    [cursor, setCursor] = useState(0),
    [draft, setDraft] = useState<string | null>(null),
    [message, setMessage] = useState("");
  const visible = useMemo(() => filterSettings(settings, query), [settings, query]);
  const current = visible[Math.min(cursor, Math.max(0, visible.length - 1))];
  const theme = dark;
  const commit = (setting: Setting, text: string) => {
    const value = parseValue(setting, text);
    client.set(setting.key, text);
    setSettings((old) => old.map((s) => (s.key === setting.key ? { ...s, value } : s)));
    setMessage(`${setting.key} = ${text}`);
  };
  useKeyboard((key) => {
    if (draft !== null) {
      if (key.name === "escape") setDraft(null);
      else if (key.name === "return") {
        try {
          commit(current, draft);
          setDraft(null);
        } catch (error) {
          setMessage(String(error instanceof Error ? error.message : error));
        }
      } else if (key.name === "backspace") setDraft(draft.slice(0, -1));
      else if (key.sequence && key.sequence.length === 1 && !key.ctrl && !key.meta) setDraft(draft + key.sequence);
      return;
    }
    if (key.name === "escape" || (key.ctrl && key.name === "c")) onQuit();
    else if (key.name === "down" || (key.ctrl && key.name === "n")) setCursor((c) => Math.min(c + 1, visible.length - 1));
    else if (key.name === "up" || (key.ctrl && key.name === "p")) setCursor((c) => Math.max(c - 1, 0));
    else if (key.name === "return" && current) {
      if (current.type === "boolean") commit(current, current.value ? "false" : "true");
      else if (current.type === "enum") {
        const options = current.options!;
        const next = options[(options.indexOf(formatValue(current.value)) + 1) % options.length];
        commit(current, next);
      } else setDraft(formatValue(current.value));
    } else if (key.name === "backspace") { setQuery((q) => q.slice(0, -1)); setCursor(0); }
    else if (key.sequence && key.sequence.length === 1 && !key.ctrl && !key.meta) { setQuery((q) => q + key.sequence); setCursor(0); }
  });
  const keyWidth = Math.min(32, Math.max(8, ...settings.map((s) => s.key.length)) + 1);
  const valueWidth = Math.min(24, Math.max(6, ...settings.map((s) => formatValue(s.value).length)) + 3);
  const descriptionWidth = Math.max(8, width - keyWidth - valueWidth - 4);
  const bodyHeight = Math.max(1, height - 4);
  const start = Math.max(0, Math.min(cursor - bodyHeight + 1, visible.length - bodyHeight));
  return (
    <box width={width} height={height} flexDirection="column" backgroundColor={theme.bg}>
      <text height={1} fg={theme.fg} selectable={false}>{fit(" diffr settings", width)}</text>
      <text height={1} fg={theme.type} selectable={false}>{fit(` > ${query}▏`, width)}</text>
      {visible.slice(start, start + bodyHeight).map((setting, i) => {
        const selected = start + i === cursor;
        const value = selected && draft !== null ? `${draft}▏` : formatValue(setting.value);
        const description = `${setting.description}${setting.default === undefined ? "" : ` (default: ${formatValue(setting.default)})`}`;
        return (
          <text key={setting.key} height={1} fg={selected ? theme.fg : theme.muted}
            bg={selected ? theme.foldBackground : theme.bg} selectable={false}>
            {` ${pad(setting.key, keyWidth)} ${pad(description, descriptionWidth)} ${pad(value, valueWidth - 2)}${isDefault(setting) ? " " : "●"}`}
          </text>
        );
      })}
      {visible.length === 0 && <text height={1} fg={theme.muted} selectable={false}>{" No settings match"}</text>}
      <box flexGrow={1} />
      <text height={1} fg={theme.muted} selectable={false}>
        {fit(draft !== null ? " enter: save · esc: cancel" : ` type to filter · ↑↓ move · enter: edit · esc: close · ● changed from default  ${message}`, width)}
      </text>
    </box>
  );
}
