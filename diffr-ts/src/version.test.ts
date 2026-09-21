import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { STRUCTURAL_DIFF_BASE_WIRE_VERSION } from "./contract.js";
import { repositoryRoot } from "./test-binary.js";

test("STRUCTURAL_DIFF_BASE_WIRE_VERSION matches src/protocol/mod.rs", () => {
  const source = readFileSync(join(repositoryRoot, "src/protocol/mod.rs"), "utf8");
  const match = /pub const VERSION: u32 = (\d+);/.exec(source);
  expect(match).not.toBeNull();
  expect(Number(match![1])).toBe(STRUCTURAL_DIFF_BASE_WIRE_VERSION);
});

test("release pins match the package version and cover both supported targets", () => {
  const root = join(repositoryRoot, "diffr-ts");
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const pins = JSON.parse(readFileSync(join(root, "pins.json"), "utf8"));
  expect(pins.version).toBe(pkg.version);
  expect(Object.keys(pins.sha256).sort()).toEqual([
    "aarch64-apple-darwin", "x86_64-unknown-linux-gnu",
  ]);
  for (const hash of Object.values(pins.sha256)) expect(hash).toMatch(/^[a-f0-9]{64}$/);
});
