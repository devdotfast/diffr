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
/** Keys that look like credentials are typed masked. */
export const isSecret = (key: string) => /(api_key|secret|token|password)/i.test(key);
export const displayValue = (value: unknown) => {
  const text = formatValue(value);
  return text === "" ? "<unset>" : text;
};
interface Edit {
  setting: Setting;
  /** Text field for numbers and strings; the chosen option for booleans and enums. */
  draft: string;
}
export function Settings({ client, onQuit, initialQuery = "" }: { client: ConfigClient; onQuit: () => void; initialQuery?: string }) {
  const { width, height } = useTerminalDimensions();
  const [settings, setSettings] = useState<Setting[]>(() => flattenSchema(client.schema(), client.show()));
  const [query, setQuery] = useState(initialQuery),
    [cursor, setCursor] = useState(0),
    [edit, setEdit] = useState<Edit | null>(null),
    [message, setMessage] = useState("");
  const visible = useMemo(() => filterSettings(settings, query), [settings, query]);
  const current = visible[Math.min(cursor, Math.max(0, visible.length - 1))];
  const theme = dark;
  const options = (setting: Setting) => (setting.type === "boolean" ? ["true", "false"] : setting.options ?? []);
  const save = (setting: Setting, text: string) => {
    const value = parseValue(setting, text);
    client.set(setting.key, text);
    setSettings((old) => old.map((s) => (s.key === setting.key ? { ...s, value } : s)));
    setMessage(`${setting.key} = ${isSecret(setting.key) ? "••••••" : displayValue(value)}`);
    setEdit(null);
  };
  useKeyboard((key) => {
    if (edit) {
      const { setting, draft } = edit;
      const choices = options(setting);
      if (key.name === "escape") setEdit(null);
      else if (key.name === "return") {
        try {
          save(setting, draft);
        } catch (error) {
          setMessage(String(error instanceof Error ? error.message : error));
        }
      } else if (choices.length) {
        const index = choices.indexOf(draft);
        if (key.name === "down" || key.name === "up" || key.name === "space" || key.name === "tab")
          setEdit({ setting, draft: choices[(index + (key.name === "up" ? choices.length - 1 : 1)) % choices.length] });
        else if (setting.type === "boolean" && (key.name === "y" || key.name === "n"))
          setEdit({ setting, draft: key.name === "y" ? "true" : "false" });
      } else if (key.name === "backspace") setEdit({ setting, draft: draft.slice(0, -1) });
      else if (key.sequence && key.sequence.length === 1 && !key.ctrl && !key.meta)
        setEdit({ setting, draft: draft + key.sequence });
      return;
    }
    if (key.name === "escape" || (key.ctrl && key.name === "c")) onQuit();
    else if (key.name === "down" || (key.ctrl && key.name === "n")) setCursor((c) => Math.min(c + 1, visible.length - 1));
    else if (key.name === "up" || (key.ctrl && key.name === "p")) setCursor((c) => Math.max(c - 1, 0));
    else if (key.name === "return" && current) {
      const choices = options(current);
      const value = formatValue(current.value);
      setEdit({ setting: current, draft: choices.length && !choices.includes(value) ? choices[0] : value });
    } else if (key.name === "backspace") { setQuery((q) => q.slice(0, -1)); setCursor(0); }
    else if (key.sequence && key.sequence.length === 1 && !key.ctrl && !key.meta) { setQuery((q) => q + key.sequence); setCursor(0); }
  });
  if (edit) {
    const { setting, draft } = edit;
    const choices = options(setting);
    const field = isSecret(setting.key) ? "•".repeat(draft.length) : draft;
    return (
      <box width={width} height={height} flexDirection="column" backgroundColor={theme.bg}>
        <text height={1} fg={theme.fg} selectable={false}>{fit(` ${setting.key}`, width)}</text>
        <text height={1} fg={theme.muted} selectable={false}>{fit(` ${setting.description}`, width)}</text>
        <text height={1} fg={theme.muted} selectable={false}>{fit(` default: ${displayValue(setting.default)}`, width)}</text>
        <text height={1} selectable={false}>{" "}</text>
        {choices.length
          ? choices.map((choice) => (
              <text key={choice} height={1} fg={choice === draft ? theme.fg : theme.muted}
                bg={choice === draft ? theme.foldBackground : theme.bg} selectable={false}>
                {fit(` ${choice === draft ? "▸" : " "} ${choice}`, width)}
              </text>
            ))
          : <text height={1} fg={theme.type} selectable={false}>{fit(` value: ${field}▏`, width)}</text>}
        <box flexGrow={1} />
        <text height={1} fg={theme.muted} selectable={false}>
          {fit(` enter save · esc back${setting.type === "boolean" ? " · y/n or ↑↓ choose" : choices.length ? " · ↑↓ choose" : ""}  ${message}`, width)}
        </text>
      </box>
    );
  }
  const keyWidth = Math.min(32, Math.max(8, ...settings.map((s) => s.key.length)) + 1);
  const valueWidth = Math.min(24, Math.max(7, ...settings.map((s) => displayValue(s.value).length)) + 3);
  const descriptionWidth = Math.max(8, width - keyWidth - valueWidth - 4);
  const bodyHeight = Math.max(1, height - 4);
  const start = Math.max(0, Math.min(cursor - bodyHeight + 1, visible.length - bodyHeight));
  return (
    <box width={width} height={height} flexDirection="column" backgroundColor={theme.bg}>
      <text height={1} fg={theme.fg} selectable={false}>{fit(" diffr settings", width)}</text>
      <text height={1} fg={theme.type} selectable={false}>{fit(` > ${query}▏`, width)}</text>
      {visible.slice(start, start + bodyHeight).map((setting, i) => {
        const selected = start + i === cursor;
        const value = isSecret(setting.key) && formatValue(setting.value) !== "" ? "••••••" : displayValue(setting.value);
        const description = `${setting.description}${setting.default === undefined ? "" : ` (default: ${displayValue(setting.default)})`}`;
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
        {fit(` ↑↓ move · enter edit · type to search · esc quit · ● changed from default  ${message}`, width)}
      </text>
    </box>
  );
}
