import { expect, test } from "bun:test";
import { resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { readDiffStream } from "../../packages/hunk/src/diffr/stream";
import { rowsForFile, dark } from "../../packages/hunk/src/diffr/rows";
const root = resolve(import.meta.dir, "../../.."),
  binary = process.env.DIFFR_TEST_BIN ?? resolve(root, "target/debug/diffr");
const before = resolve(import.meta.dir, "../fixtures/before.ts"),
  after = resolve(import.meta.dir, "../fixtures/after.ts");
test.skipIf(process.platform === "win32")(
  "actual CLI terminal lifecycle and mouse controls",
  () => {
    const result = spawnSync(
      "python3",
      [
        resolve(import.meta.dir, "launch.py"),
        binary,
        process.execPath,
        before,
        after,
      ],
      { encoding: "utf8", timeout: 25000 },
    );
    expect(result.stderr).toBe("");
    expect(result.status).toBe(0);
  },
  30000,
);
test("actual Rust output satisfies wire schema and renders both layouts", async () => {
  const process = Bun.spawn(
    [binary, "--no-index", "--format", "ndjson", "--syntax", "--", before, after],
    { stdout: "pipe", stderr: "pipe" },
  );
  const events = await Array.fromAsync(readDiffStream(process.stdout));
  expect(await process.exited).toBe(0);
  const file = events.find((e) => e.type === "file");
  expect(file?.type).toBe("file");
  if (file?.type !== "file" || !file.diff) throw new Error("diffr did not produce a diff");
  const loaded = { ...file, diff: file.diff };
  expect(rowsForFile(loaded, 0, "split", dark).some((r) => r.left)).toBe(true);
  expect(rowsForFile(loaded, 0, "unified", dark).some((r) => r.cell)).toBe(true);
  if (file.diff.type !== "text") throw new Error("fixture diff is not text");
  // --syntax fills capture names; the fixture is TypeScript so keywords are present.
  expect(file.diff.rhs!.syntax.some((span) => span.capture.startsWith("keyword"))).toBe(true);
  expect(file.diff.rhs!.regions.length).toBeGreaterThan(0);
});
test("redirected output and explicit text bypass the TUI even without Bun", () => {
  for (const format of [[], ["--format", "text"]]) {
    const result = spawnSync(
      binary,
      ["--no-index", ...format, "--", before, after],
      {
        encoding: "utf8",
        env: { ...process.env, DIFFR_BUN: "nonexistent-bun-for-test" },
      },
    );
    expect(result.status).toBe(1);
    expect(result.stdout).not.toContain("\x1b[?1049h");
    expect(result.stderr).not.toContain("launch terminal");
  }
});

test.skipIf(process.platform === "win32")(
  "interactive exit-code preserves the comparison status",
  () => {
    const result = spawnSync(
      "python3",
      [
        resolve(import.meta.dir, "launch.py"),
        binary,
        process.execPath,
        before,
        after,
      ],
      {
        encoding: "utf8",
        timeout: 25000,
        env: { ...process.env, DIFFR_TEST_EXIT_CODE: "1" },
      },
    );
    expect(result.stderr).toBe("");
    expect(result.status).toBe(0);
  },
  30000,
);
