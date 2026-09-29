// Exercise the public downloader with exact staged release bytes, then switch executables.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { ensureBinary } from "../diffr-ts/bin/binary.mjs";

const dist = resolve(process.argv[2]);
const fixtures = resolve(process.argv[3]);
const { version } = JSON.parse(readFileSync(new URL("../diffr-ts/package.json", import.meta.url)));
const targets = { "darwin-arm64": "aarch64-apple-darwin", "darwin-x64": "x86_64-apple-darwin", "linux-arm64": "aarch64-unknown-linux-gnu", "linux-x64": "x86_64-unknown-linux-gnu" };
const target = targets[`${process.platform}-${process.arch}`];
assert.ok(target);
const directory = mkdtempSync(join(tmpdir(), "diffr-editions-"));
try {
  const editions = {};
  for (const [edition, artifact] of [["lean", "diffr-cli"], ["full", "diffr-cli-full"]]) {
    const bytes = readFileSync(join(dist, `${artifact}-${version}-${target}.tar.gz`));
    editions[edition] = { artifact, sha256: { [target]: createHash("sha256").update(bytes).digest("hex") } };
  }
  const pins = join(directory, "pins.json");
  writeFileSync(pins, JSON.stringify({ version, editions }));
  const options = { directory, pins };
  globalThis.fetch = async (url) => {
    assert.ok(url.startsWith(`https://github.com/devdotfast/diffr/releases/download/${version}/`));
    return new Response(readFileSync(join(dist, basename(url))));
  };
  const lean = await ensureBinary(options);
  const full = await ensureBinary({ ...options, edition: "full" });
  assert.notEqual(lean, full);
  globalThis.fetch = () => { throw new Error("network disabled"); };
  assert.equal(await ensureBinary({ ...options, check: true }), lean);
  assert.equal(await ensureBinary({ ...options, edition: "full", check: true }), full);

  const env = { PATH: "", HOME: directory, XDG_CONFIG_HOME: directory };
  const run = (binary, args) => {
    const result = spawnSync(binary, args, { env, cwd: directory, encoding: "utf8", maxBuffer: 16 * 1024 * 1024 });
    assert.equal(result.status, 0, result.stderr);
    return result.stdout;
  };
  for (const [prefix, extension] of [["apex", "trigger"], ["fortran", "f90"], ["f_sharp", "fs"], ["haskell", "hs"], ["julia", "jl"], ["ocaml", "ml"], ["ocaml_interface", "mli"], ["qml", "qml"], ["verilog", "sv"], ["vhdl", "vhd"]]) {
    const args = ["--no-index", "--format", "ndjson", "--syntax", "--", join(fixtures, `${prefix}_1.${extension}`), join(fixtures, `${prefix}_2.${extension}`)];
    // Switch full -> lean -> full in one Node process with no application restart.
    for (const binary of [full, lean, full]) {
      const events = run(binary, args).trim().split("\n").map(JSON.parse);
      assert.equal(events.at(-1).type, "complete");
      assert.equal(events.at(-1).failed, 0);
      const file = events.find(event => event.type === "file");
      assert.equal(file.diff.stats.fallback?.code, binary === lean ? "unsupported_language" : undefined, prefix);
    }
  }
  const common = ["--no-index", "--format", "ndjson", "--syntax", "--", join(fixtures, "comments_1.rs"), join(fixtures, "comments_2.rs")];
  assert.equal(run(lean, common), run(full, common), "bundled Rust output must match across editions");
  console.log(`PASS: ${target} lean/full installs, offline reuse and ten-language runtime switching`);
} finally {
  rmSync(directory, { recursive: true, force: true });
}
