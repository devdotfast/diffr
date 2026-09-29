import { expect, test } from "bun:test";
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

for (const failLast of [false, true]) {
  test(`pinning ${failLast ? "preserves the previous catalog on partial publication" : "requires all eight edition archives"}`, () => {
    const root = mkdtempSync(join(tmpdir(), "diffr-pin-"));
    try {
      mkdirSync(join(root, "scripts"));
      cpSync(join(import.meta.dir, "../scripts/pin.mjs"), join(root, "scripts/pin.mjs"));
      writeFileSync(join(root, "package.json"), JSON.stringify({ version: "1.2.3" }));
      writeFileSync(join(root, "pins.json"), "previous pins");
      writeFileSync(join(root, "mock.mjs"), `
        import assert from "node:assert/strict";
        let count = 0;
        globalThis.fetch = async (url) => {
          count++;
          assert.ok(url.includes(count <= 4 ? "/diffr-cli-1.2.3-" : "/diffr-cli-full-1.2.3-"));
          return new Response("archive", { status: ${failLast} && count === 8 ? 404 : 200 });
        };
        process.on("exit", () => assert.equal(count, 8));
      `);
      const result = Bun.spawnSync(["node", "--import", join(root, "mock.mjs"), join(root, "scripts/pin.mjs")]);
      expect(result.exitCode, result.stderr.toString()).toBe(failLast ? 1 : 0);
      const contents = readFileSync(join(root, "pins.json"), "utf8");
      if (failLast) expect(contents).toBe("previous pins");
      else {
        const pins = JSON.parse(contents);
        expect(Object.keys(pins.editions).sort()).toEqual(["full", "lean"]);
        expect(pins.editions.full.artifact).toBe("diffr-cli-full");
        for (const edition of ["lean", "full"]) expect(Object.keys(pins.editions[edition].sha256)).toHaveLength(4);
      }
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
}
