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
  /** Where the line sits in `patchTexts`' before and after text; -1 on a side it is not on. */
  at: [number, number];
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
  let oldLine = 0, newLine = 0, oldAt = 0, newAt = 0;
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
    if (sign === "+") hunk.lines.push({ kind: "add", text, new: newLine++, at: [-1, newAt++] });
    else if (sign === "-") hunk.lines.push({ kind: "del", text, old: oldLine++, at: [oldAt++, -1] });
    else if (sign === " " || (sign === undefined && oldLine < hunk.oldStart + hunk.oldLines))
      hunk.lines.push({ kind: "context", text, old: oldLine++, new: newLine++, at: [oldAt++, newAt++] });
    // "\ No newline at end of file" and anything else carry no line.
  }
  return hunks;
}

/**
 * The lines the patch shows of each side, joined, for highlighting: tree-sitter reads the hunks as
 * one fragment of the file, which colours them as GitHub does, near enough, until diffr has the file.
 */
export function patchTexts(hunks: Hunk[]): [string, string] {
  const sides: [string[], string[]] = [[], []];
  for (const hunk of hunks)
    for (const line of hunk.lines) {
      if (line.at[0] >= 0) sides[0].push(line.text);
      if (line.at[1] >= 0) sides[1].push(line.text);
    }
  return [sides[0].join("\n"), sides[1].join("\n")];
}

/** The highest line number the patch shows, for the gutter's width. */
export function lastLine(hunks: Hunk[]) {
  const last = hunks.at(-1);
  return last ? Math.max(last.oldStart + last.oldLines, last.newStart + last.newLines) : 0;
}
