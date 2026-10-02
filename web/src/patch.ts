/**
 * The size of GitHub's patch for a file, from the file listing the page already loads. A file
 * holds this much room until diffr has diffed it, so the scrollbar is close to true from the start.
 */
export interface PatchSize {
  /** Rows the patch's lines take in the layout. */
  lines: number;
  /** Stretches between and around hunks the patch leaves out. */
  gaps: number;
}

const header = /^@@ -(\d+)/;

export function patchSize(patch: string, split: boolean): PatchSize {
  let lines = 0, gaps = 0, dels = 0, adds = 0;
  const flush = () => {
    lines += split ? Math.max(dels, adds) : dels + adds;
    dels = adds = 0;
  };
  for (const line of patch.split("\n")) {
    const match = header.exec(line);
    if (match) {
      flush();
      if (gaps || Number(match[1]) > 1) gaps++;
      continue;
    }
    if (line[0] === "-") dels++;
    else if (line[0] === "+") {
      adds++;
      if (!split) flush();
    } else if (line[0] !== "\\") {
      flush();
      lines++;
    }
  }
  flush();
  return { lines, gaps };
}
