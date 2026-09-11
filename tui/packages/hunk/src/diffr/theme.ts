/** Load Helix themes (TOML keyed by tree-sitter capture names) into the painter's palette. */
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
export interface Style {
  fg?: string;
  bg?: string;
  modifiers: string[];
}
export interface HelixTheme {
  name: string;
  styles: Map<string, Style>;
  palette: Map<string, string>;
}
/** Colours the row painter reads; syntax colours come from the theme by capture name. */
export interface Palette {
  name: string;
  isLight: boolean;
  bg: string;
  fg: string;
  muted: string;
  /** Header, menu, and sidebar chrome. */
  chrome: string;
  /** Sidebar highlight for the active file. */
  highlight: string;
  addition: string;
  deletion: string;
  addWord: string;
  deleteWord: string;
  addedText: string;
  removedText: string;
  /** An accent for interactive text such as links and the layout badge. */
  accent: string;
  /** VS Code's editor.foldBackground and foldPlaceholderForeground. */
  foldBackground: string;
  foldPlaceholder: string;
  /** Foreground for a tree-sitter capture such as `keyword.return`; undefined when the theme has no scope for it. */
  syntax: (capture: string) => string | undefined;
}
/** Helix's named terminal colours, used when a theme writes `fg = "red"` outside its palette. */
const ansi: Record<string, string> = {
  black: "#000000", red: "#cd3131", green: "#0dbc79", yellow: "#e5e510", blue: "#2472c8",
  magenta: "#bc3fbc", cyan: "#11a8cd", white: "#e5e5e5", gray: "#666666", grey: "#666666",
  "light-red": "#f14c4c", "light-green": "#23d18b", "light-yellow": "#f5f543", "light-blue": "#3b8eea",
  "light-magenta": "#d670d6", "light-cyan": "#29b8db", "light-white": "#ffffff", "light-gray": "#a0a0a0",
  "light-grey": "#a0a0a0", "light-black": "#333333",
};
interface RawStyle {
  fg?: string;
  bg?: string;
  modifiers?: string[];
}
export function parseHelixTheme(text: string, name: string): HelixTheme {
  const raw = Bun.TOML.parse(text) as Record<string, unknown>;
  const palette = new Map<string, string>();
  const rawPalette = raw.palette;
  if (rawPalette && typeof rawPalette === "object")
    for (const [key, value] of Object.entries(rawPalette as Record<string, unknown>)) {
      if (typeof value !== "string") throw new Error(`Theme ${name}: palette entry ${key} is not a colour`);
      palette.set(key, value);
    }
  const styles = new Map<string, Style>();
  for (const [scope, value] of Object.entries(raw)) {
    if (scope === "palette" || scope === "inherits") continue;
    if (typeof value === "string") styles.set(scope, { fg: value, modifiers: [] });
    else if (value && typeof value === "object") {
      const style = value as RawStyle;
      styles.set(scope, { fg: style.fg, bg: style.bg, modifiers: style.modifiers ?? [] });
    } else throw new Error(`Theme ${name}: scope ${scope} has an unsupported value`);
  }
  if (typeof raw.inherits === "string")
    throw new Error(`Theme ${name} inherits from ${raw.inherits}; inherited themes are not supported`);
  return { name, styles, palette };
}
/** A palette name, a hex colour, or one of Helix's terminal colour names. */
export function colorOf(theme: HelixTheme, value: string): string {
  const fromPalette = theme.palette.get(value);
  if (fromPalette !== undefined) return colorOf(theme, fromPalette);
  if (/^#[0-9a-fA-F]{6}$/.test(value)) return value;
  const named = ansi[value];
  if (named) return named;
  throw new Error(`Theme ${theme.name}: unknown colour ${value}`);
}
/** The style for a scope, falling back to its parent scopes: `keyword.return` → `keyword`. */
export function scopeStyle(theme: HelixTheme, scope: string): Style | undefined {
  const parts = scope.split(".");
  while (parts.length) {
    const style = theme.styles.get(parts.join("."));
    if (style) return style;
    parts.pop();
  }
  return undefined;
}
const scopeFg = (theme: HelixTheme, scope: string) => {
  const style = scopeStyle(theme, scope);
  return style?.fg === undefined ? undefined : colorOf(theme, style.fg);
};
const scopeBg = (theme: HelixTheme, scope: string) => {
  const style = scopeStyle(theme, scope);
  return style?.bg === undefined ? undefined : colorOf(theme, style.bg);
};
function luminance(hex: string) {
  const channel = (i: number) => parseInt(hex.slice(i, i + 2), 16) / 255;
  return 0.2126 * channel(1) + 0.7152 * channel(3) + 0.0722 * channel(5);
}
function mix(hex: string, other: string, amount: number) {
  const channel = (i: number) =>
    Math.round(parseInt(hex.slice(i, i + 2), 16) * (1 - amount) + parseInt(other.slice(i, i + 2), 16) * amount);
  return `#${[1, 3, 5].map((i) => channel(i).toString(16).padStart(2, "0")).join("")}`;
}
/** Build the painter's palette: chrome from `ui.*`, syntax by capture, change tints mixed into the background. */
export function paletteFromHelix(theme: HelixTheme): Palette {
  const bg = scopeBg(theme, "ui.background");
  const fg = scopeFg(theme, "ui.text");
  if (!bg || !fg) throw new Error(`Theme ${theme.name} lacks ui.background or ui.text`);
  const isLight = luminance(bg) > 0.5;
  const muted = scopeFg(theme, "ui.linenr") ?? scopeFg(theme, "comment") ?? mix(fg, bg, 0.4);
  const plus = scopeFg(theme, "diff.plus") ?? (isLight ? "#1a7f37" : "#7ee787");
  const minus = scopeFg(theme, "diff.minus") ?? (isLight ? "#cf222e" : "#ffa198");
  return {
    name: theme.name,
    isLight,
    bg,
    fg,
    muted,
    chrome: scopeBg(theme, "ui.statusline") ?? mix(bg, fg, 0.06),
    highlight: scopeBg(theme, "ui.selection") ?? mix(bg, fg, 0.15),
    addition: mix(bg, plus, 0.18),
    deletion: mix(bg, minus, 0.18),
    addWord: mix(bg, plus, 0.42),
    deleteWord: mix(bg, minus, 0.42),
    addedText: plus,
    removedText: minus,
    accent: scopeFg(theme, "function") ?? scopeFg(theme, "ui.text.focus") ?? fg,
    foldBackground: scopeBg(theme, "ui.cursorline.primary") ?? mix(bg, fg, 0.1),
    foldPlaceholder: muted,
    syntax: (capture) => scopeFg(theme, capture),
  };
}
/** Bundled Helix themes under tui/themes; the two defaults are aliases. */
export const bundledThemes: Record<string, string> = {
  "default-dark": "onedark",
  "default-light": "onelight",
  onedark: "onedark",
  onelight: "onelight",
  gruvbox: "gruvbox",
  solarized_light: "solarized_light",
};
const themesDir = resolve(import.meta.dir, "../../../../themes");
export function loadBundledTheme(name: string): Palette {
  const file = bundledThemes[name];
  if (!file) throw new Error(`Unknown theme ${name}; bundled themes: ${Object.keys(bundledThemes).join(", ")}`);
  return paletteFromHelix(parseHelixTheme(readFileSync(resolve(themesDir, `${file}.toml`), "utf8"), name));
}
export function loadThemeFile(path: string): Palette {
  return paletteFromHelix(parseHelixTheme(readFileSync(path, "utf8"), path));
}
export interface ThemeSet {
  initial: Palette;
  dark: Palette;
  light: Palette;
}
/** Resolve the theme diffr's config names, plus the two defaults the `t` key toggles between. */
export function themesFromConfig(config: { name: string; path: string | null }): ThemeSet {
  const dark = loadBundledTheme("default-dark"), light = loadBundledTheme("default-light");
  const initial = config.path ? loadThemeFile(config.path) : loadBundledTheme(config.name);
  return { initial, dark, light };
}
export function themeConfig(show: unknown): { name: string; path: string | null } {
  const theme = (show as { theme?: { name?: unknown; path?: unknown } }).theme;
  if (!theme || typeof theme.name !== "string" || (theme.path !== null && theme.path !== undefined && typeof theme.path !== "string"))
    throw new Error("diffr config show did not include a theme section");
  return { name: theme.name, path: theme.path ?? null };
}
