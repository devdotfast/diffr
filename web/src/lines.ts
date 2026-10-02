/**
 * A file's text with where each line starts, so a million-line change is a few strings and offset
 * arrays rather than a string per line. Plain data, so a worker can hand it over.
 */
export interface Lines {
  text: string;
  /** Where each line starts, then one past the last line's end, as if every line ended in a newline. */
  starts: Uint32Array;
}

export function splitLines(text: string): Lines {
  if (!text) return { text, starts: new Uint32Array([0]) };
  const starts: number[] = [0];
  for (let i = text.indexOf("\n"); i !== -1; i = text.indexOf("\n", i + 1)) starts.push(i + 1);
  if (!text.endsWith("\n")) starts.push(text.length + 1);
  return { text, starts: Uint32Array.from(starts) };
}

export const lineCount = (lines: Lines) => lines.starts.length - 1;

export const lineAt = (lines: Lines, index: number) =>
  index < lines.starts.length - 1 ? lines.text.slice(lines.starts[index], lines.starts[index + 1]! - 1) : "";
