/**
 * Syntax colour for diffr's tree-sitter captures, in Pierre's palette (pierre-dark-soft and
 * pierre-light, from @pierre/theme): each capture becomes a class that sets the
 * `--diffs-token-dark` / `--diffs-token-light` pair the diffs stylesheet paints line spans with.
 */
import type { Span } from "../../tui/packages/hunk/src/diffr/wire";
import { lineSpans, type Syntax } from "./syntax";

/** [dark, light] per token kind. */
const palette = {
  comment: ["#636363", "#737373"],
  string: ["#8cda94", "#199f43"],
  regexp: ["#92dde4", "#17a5af"],
  escape: ["#8fe0d0", "#16a994"],
  number: ["#96d9f6", "#1ca1c7"],
  constant: ["#ffbc56", "#d5901c"],
  keyword: ["#ff91a8", "#d32a61"],
  operator: ["#68cdf2", "#08c0ef"],
  punctuation: ["#737373", "#636363"],
  variable: ["#ffba82", "#d47628"],
  builtin: ["#ffbc56", "#d5901c"],
  parameter: ["#a3a3a3", "#636363"],
  function: ["#ba8ffd", "#693acf"],
  type: ["#e290f0", "#a631be"],
  module: ["#ffbc56", "#d5901c"],
  tag: ["#ffa685", "#d5512f"],
  attribute: ["#8eddb2", "#18a46c"],
  decorator: ["#97c4ff", "#1a85d4"],
  heading: ["#ffa685", "#d5512f"],
  emphasis: ["#ffde80", "#d5a910"],
} as const;
export type Token = keyof typeof palette;

/** Most specific first: a capture takes the first rule its dotted name starts with. */
const rules: [string, Token][] = [
  ["comment", "comment"],
  ["string.regex", "regexp"],
  ["string.escape", "escape"],
  ["string.special.path", "string"],
  ["string", "string"],
  ["character", "string"],
  ["escape", "escape"],
  ["number", "number"],
  ["float", "number"],
  ["boolean", "number"],
  ["constant.builtin", "number"],
  ["constant", "constant"],
  ["keyword.operator", "keyword"],
  ["keyword", "keyword"],
  ["conditional", "keyword"],
  ["repeat", "keyword"],
  ["include", "keyword"],
  ["exception", "keyword"],
  ["storageclass", "keyword"],
  ["preproc", "keyword"],
  ["define", "keyword"],
  ["operator", "operator"],
  ["punctuation.special", "operator"],
  ["punctuation", "punctuation"],
  ["variable.parameter", "parameter"],
  ["parameter", "parameter"],
  ["variable.builtin", "builtin"],
  ["variable", "variable"],
  ["property", "variable"],
  ["field", "variable"],
  ["label", "builtin"],
  ["function.macro", "function"],
  ["function", "function"],
  ["method", "function"],
  ["constructor", "type"],
  ["type", "type"],
  ["module", "module"],
  ["namespace", "module"],
  ["tag.attribute", "attribute"],
  ["tag", "tag"],
  ["attribute", "decorator"],
  ["markup.heading", "heading"],
  ["text.title", "heading"],
  ["markup.strong", "emphasis"],
  ["markup.italic", "keyword"],
  ["markup.link", "string"],
  ["markup.raw", "string"],
  ["text.literal", "string"],
  ["text.uri", "string"],
];

const cache = new Map<string, Token | null>();
export function tokenOf(capture: string): Token | null {
  let token = cache.get(capture);
  if (token !== undefined) return token;
  token = null;
  for (const [prefix, kind] of rules)
    if (capture === prefix || capture.startsWith(prefix + ".")) {
      token = kind;
      break;
    }
  cache.set(capture, token);
  return token;
}

/** Classes for every token, to adopt beside the diffs stylesheet. */
export const tokenCss = Object.entries(palette)
  .map(([name, [dark, light]]) => `.t-${name}{--diffs-token-dark:${dark};--diffs-token-light:${light}}`)
  .join("\n");

const escapes: Record<string, string> = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" };
const escape = (text: string) => text.replace(/[&<>"]/g, (c) => escapes[c]!);
const ascii = /^[\x00-\x7f]*$/;

/** UTF-16 index for each byte offset diffr sends, for a line with multibyte characters. */
function byteToIndex(text: string): (byte: number) => number {
  if (ascii.test(text)) return (byte) => Math.min(byte, text.length);
  const offsets: number[] = [];
  let byte = 0;
  for (let i = 0; i < text.length; i++) {
    const code = text.codePointAt(i)!;
    const width = code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
    for (let k = 0; k < width; k++) offsets[byte + k] = i;
    byte += width;
    if (code >= 0x10000) i++;
  }
  offsets[byte] = text.length;
  return (b) => offsets[Math.min(b, byte)] ?? text.length;
}

/**
 * The runs to emphasize: diffr marks tokens, so neighbours with only spaces between them become one
 * run, and a line changed from end to end gets none, since its background already says so.
 */
function emphasis(text: string, marks: (readonly [number, number])[]): (readonly [number, number])[] {
  if (!marks.length) return marks;
  const sorted = [...marks].sort((a, b) => a[0] - b[0]);
  const runs: [number, number][] = [];
  for (const [start, end] of sorted) {
    const last = runs.at(-1);
    if (last && (start <= last[1] || !text.slice(last[1], start).trim())) last[1] = Math.max(last[1], end);
    else runs.push([start, end]);
  }
  const first = text.search(/\S/), end = text.trimEnd().length;
  if (runs.length === 1 && runs[0]![0] <= first && runs[0]![1] >= end) return [];
  return runs;
}

/**
 * A line as HTML: tree-sitter tokens, with the spans diffr found changed wrapped in
 * `data-diff-span` so the diffs stylesheet gives them the emphasis tint.
 */
export function lineHtml(text: string, packed: Syntax | undefined, line: number, changed: Span[]): string {
  if (!text) return "";
  const syntax = lineSpans(packed, line);
  if (!syntax.length && !changed.length) return escape(text);
  const index = byteToIndex(text);
  const bounds = new Set([0, text.length]);
  const tokens: { start: number; end: number; token: Token }[] = [];
  for (const [startByte, endByte, capture] of syntax) {
    const token = tokenOf(capture);
    if (!token) continue;
    const start = index(startByte), end = index(endByte);
    if (end <= start) continue;
    tokens.push({ start, end, token });
    bounds.add(start);
    bounds.add(end);
  }
  const marks = emphasis(text, changed.map((span) => [index(span.start_column), index(span.end_column)] as const));
  for (const [start, end] of marks) {
    bounds.add(start);
    bounds.add(end);
  }
  const cuts = [...bounds].filter((b) => b <= text.length).sort((a, b) => a - b);
  let html = "";
  let open = false;
  for (let i = 0; i + 1 < cuts.length; i++) {
    const start = cuts[i]!, end = cuts[i + 1]!;
    // The innermost capture wins: the narrowest one covering this piece.
    let best: (typeof tokens)[number] | undefined;
    for (const t of tokens)
      if (t.start <= start && t.end >= end && (!best || t.end - t.start < best.end - best.start)) best = t;
    const emphasized = marks.some(([s, e]) => s <= start && e >= end);
    if (emphasized && !open) {
      html += "<span data-diff-span>";
      open = true;
    } else if (!emphasized && open) {
      html += "</span>";
      open = false;
    }
    const piece = escape(text.slice(start, end));
    html += best ? `<span class="t-${best.token}">${piece}</span>` : piece;
  }
  if (open) html += "</span>";
  return html;
}
