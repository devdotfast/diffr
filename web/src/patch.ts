/**
 * GitHub's own patch for a file, from the file listing the page already loads: shown at once, so
 * every file reads as a line diff before diffr has fetched and diffed it.
 */
export interface PatchLine {
  kind: "context" | "add" | "del";
  text: string;
  /** 1-based line numbers on the sides the line is on. */
  old?: number;
  new?: number;
}

export interface Hunk {
  oldStart: number;
  oldLines: number;
  newStart: number;
  newLines: number;
  lines: PatchLine[];
}

const header = /^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/;

export function parsePatch(patch: string): Hunk[] {
  const hunks: Hunk[] = [];
  let hunk: Hunk | undefined;
  let oldLine = 0, newLine = 0;
  for (const line of patch.split("\n")) {
    const match = header.exec(line);
    if (match) {
      hunk = {
        oldStart: Number(match[1]), oldLines: match[2] === undefined ? 1 : Number(match[2]),
        newStart: Number(match[3]), newLines: match[4] === undefined ? 1 : Number(match[4]),
        lines: [],
      };
      hunks.push(hunk);
      oldLine = hunk.oldStart;
      newLine = hunk.newStart;
      continue;
    }
    if (!hunk) continue;
    const sign = line[0], text = line.slice(1);
    if (sign === "+") hunk.lines.push({ kind: "add", text, new: newLine++ });
    else if (sign === "-") hunk.lines.push({ kind: "del", text, old: oldLine++ });
    else if (sign === " " || (sign === undefined && oldLine < hunk.oldStart + hunk.oldLines))
      hunk.lines.push({ kind: "context", text, old: oldLine++, new: newLine++ });
    // "\ No newline at end of file" and anything else carry no line.
  }
  return hunks;
}

/** The highest line number the patch shows, for the gutter's width. */
export function lastLine(hunks: Hunk[]) {
  const last = hunks.at(-1);
  return last ? Math.max(last.oldStart + last.oldLines, last.newStart + last.newLines) : 0;
}
