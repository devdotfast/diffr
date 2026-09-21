import { afterEach, expect, test } from "bun:test";
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const fetchScript = join(import.meta.dir, "..", "bin", "fetch.mjs");
const { version } = JSON.parse(readFileSync(join(import.meta.dir, "..", "package.json"), "utf8"));
const targets: Record<string, string> = { "darwin-arm64": "aarch64-apple-darwin", "linux-x64": "x86_64-unknown-linux-gnu" };
const target = targets[`${process.platform}-${process.arch}`];
const dirs: string[] = [];
afterEach(() => {
  for (const dir of dirs.splice(0)) rmSync(dir, { recursive: true, force: true });
});

function fixture(entry = "diffr") {
  if (!target) throw new Error("Fetch tests require a supported release platform");
  const dir = mkdtempSync(join(tmpdir(), "diffr-fetch-test-"));
  dirs.push(dir);
  const content = "#!/bin/sh\necho diffr 0.1.1\n";
  writeFileSync(join(dir, entry), content);
  const archive = join(dir, "archive.tar.gz");
  expect(Bun.spawnSync(["tar", "-czf", archive, "-C", dir, entry]).exitCode).toBe(0);
  const hash = createHash("sha256").update(readFileSync(archive)).digest("hex");
  const pins = join(dir, "pins.json");
  writeFileSync(pins, JSON.stringify({ version, sha256: { [target]: hash } }));
  const mock = join(dir, "mock.mjs");
  // Intercept only the network boundary, leaving the actual CLI and tar intact.
  writeFileSync(mock, `
    import { readFileSync, writeFileSync } from "node:fs";
    globalThis.fetch = async (url) => {
      writeFileSync(${JSON.stringify(join(dir, "requested"))}, url);
      if (process.env.FETCH_FAILURE) throw new Error("offline");
      return new Response(readFileSync(${JSON.stringify(archive)}));
    };
  `);
  const into = join(dir, "bin");
  function run(args: string[] = [], offline = false) {
    const result = Bun.spawnSync(["node", "--import", mock, fetchScript, "--into", into, "--pins", pins, ...args], {
      env: { ...process.env, FETCH_FAILURE: offline ? "1" : "" },
    });
    return { code: result.exitCode, out: result.stdout.toString(), err: result.stderr.toString() };
  }
  return { dir, into, pins, content, run };
}

test("downloads, verifies, extracts, stamps, and reuses the pinned binary", () => {
  const f = fixture();
  const result = f.run(["--required"]);
  expect(result.code, result.err).toBe(0);
  expect(readFileSync(join(f.dir, "requested"), "utf8")).toBe(
    `https://github.com/devdotfast/diffr/releases/download/${version}/diffr-${version}-${target}.tar.gz`,
  );
  expect(readFileSync(join(f.into, "diffr"), "utf8")).toBe(f.content);
  expect(statSync(join(f.into, "diffr")).mode & 0o777).toBe(0o755);
  expect(JSON.parse(readFileSync(join(f.into, "diffr.stamp.json"), "utf8"))).toEqual({ version, target });
  expect(readdirSync(f.into).sort()).toEqual(["diffr", "diffr.stamp.json"]);
  rmSync(join(f.dir, "requested"));
  expect(f.run(["--check"], true).code).toBe(0);
  expect(f.run([], true).code).toBe(0);
  expect(existsSync(join(f.dir, "requested"))).toBe(false);
});

test("--check never downloads and rejects missing, stale, damaged, or nonexecutable installs", () => {
  const f = fixture();
  expect(f.run(["--check"]).code).toBe(1);
  expect(existsSync(join(f.dir, "requested"))).toBe(false);
  expect(f.run().code).toBe(0);
  for (const stamp of [JSON.stringify({ version: "0.0.0", target }), JSON.stringify({ version, target: "wrong" }), "{"]) {
    writeFileSync(join(f.into, "diffr.stamp.json"), stamp);
    expect(f.run(["--check"]).code).toBe(1);
  }
  expect(f.run().code).toBe(0);
  chmodSync(join(f.into, "diffr"), 0o644);
  expect(f.run(["--check"]).code).toBe(1);
});

test("mismatched pin versions and invalid hashes fail before downloading", () => {
  const f = fixture();
  writeFileSync(f.pins, JSON.stringify({ version: "9.9.9", sha256: {} }));
  expect(f.run().err).toContain("pins.json is for 9.9.9");
  writeFileSync(f.pins, JSON.stringify({ version, sha256: {} }));
  expect(f.run().err).toContain("no valid sha256");
  expect(existsSync(join(f.dir, "requested"))).toBe(false);
});

test("network failure warns unless --required", () => {
  const f = fixture();
  const optional = f.run([], true);
  expect(optional.code).toBe(0);
  expect(optional.err).toContain("continuing without a bundled diffr");
  expect(f.run(["--required"], true).code).toBe(1);
  expect(existsSync(f.into)).toBe(false);
});

test("hash mismatch always fails and preserves the previous installation", () => {
  const f = fixture();
  expect(f.run().code).toBe(0);
  const stamp = JSON.stringify({ version: "0.0.0", target });
  writeFileSync(join(f.into, "diffr.stamp.json"), stamp);
  writeFileSync(f.pins, JSON.stringify({ version, sha256: { [target!]: "0".repeat(64) } }));
  const result = f.run();
  expect(result.code).toBe(1);
  expect(result.err).toContain("does not match pinned");
  expect(readFileSync(join(f.into, "diffr"), "utf8")).toBe(f.content);
  expect(readFileSync(join(f.into, "diffr.stamp.json"), "utf8")).toBe(stamp);
});

test("missing archive entry fails and cleans staging", () => {
  const f = fixture("other");
  const result = f.run();
  expect(result.code).toBe(1);
  expect(result.err).toContain("does not contain a regular diffr file");
  expect(readdirSync(f.into)).toEqual([]);
});

test("missing option values fail clearly", () => {
  const f = fixture();
  expect(f.run(["--pins"]).code).toBe(1);
  expect(f.run(["--into"]).code).toBe(1);
});
