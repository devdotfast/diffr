#!/usr/bin/env node
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { chmodSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { parseArgs } from "node:util";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const targets = { "darwin-arm64": "aarch64-apple-darwin", "linux-x64": "x86_64-unknown-linux-gnu" };

try {
  await main();
} catch (error) {
  console.error(`diffr-fetch: ${error.message}`);
  process.exitCode = 1;
}

async function main() {
  const { values } = parseArgs({ options: {
    into: { type: "string" },
    check: { type: "boolean" },
    required: { type: "boolean" },
    pins: { type: "string", default: join(root, "pins.json") },
  } });
  if (!values.into) throw new Error("--into <dir> is required");
  const into = resolve(values.into);
  const { version } = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const pins = JSON.parse(readFileSync(values.pins, "utf8"));
  if (pins.version !== version) throw new Error(`pins.json is for ${pins.version}, package is ${version}; run \`npm run pin\``);
  const target = targets[`${process.platform}-${process.arch}`];
  if (!target) throw new Error(`no diffr release for ${process.platform}-${process.arch}`);
  const expected = pins.sha256?.[target];
  if (typeof expected !== "string" || !/^[a-f0-9]{64}$/.test(expected)) {
    throw new Error(`pins.json has no valid sha256 for ${target}`);
  }

  const binary = join(into, "diffr");
  const stampPath = join(into, "diffr.stamp.json");
  let installed = false;
  try {
    const stamp = JSON.parse(readFileSync(stampPath, "utf8"));
    const stat = lstatSync(binary);
    installed = stat.isFile() && (stat.mode & 0o111) !== 0 && stamp.version === version && stamp.target === target;
  } catch { /* Missing or damaged installations are fetched again. */ }
  if (installed) {
    console.log(`diffr ${version} (${target}) already at ${binary}`);
    return;
  }
  if (values.check) throw new Error(`${binary} is missing or not diffr ${version} (${target})`);

  const asset = `diffr-${version}-${target}.tar.gz`;
  const url = `https://github.com/devdotfast/diffr/releases/download/${version}/${asset}`;
  let bytes;
  try {
    const response = await fetch(url, { redirect: "follow", signal: AbortSignal.timeout(120_000) });
    if (!response.ok) throw new Error(`HTTP ${response.status} ${response.statusText}`);
    bytes = Buffer.from(await response.arrayBuffer());
  } catch (error) {
    if (values.required) throw new Error(`could not download ${url}: ${error.message}`);
    console.warn(`diffr-fetch: could not download ${url}: ${error.message}; continuing without a bundled diffr`);
    return;
  }
  const actual = createHash("sha256").update(bytes).digest("hex");
  if (actual !== expected) throw new Error(`${asset} sha256 ${actual} does not match pinned ${expected}`);

  mkdirSync(into, { recursive: true });
  const staging = mkdtempSync(join(into, ".diffr-"));
  try {
    const archive = join(staging, "archive.tar.gz");
    writeFileSync(archive, bytes);
    const tar = spawnSync("tar", ["-xzf", archive, "-C", staging, "diffr"], { encoding: "utf8" });
    const extracted = join(staging, "diffr");
    if (tar.status !== 0 || !lstatSync(extracted).isFile()) {
      throw new Error(`${asset} does not contain a regular diffr file at its root`);
    }
    chmodSync(extracted, 0o755);
    const stamp = join(staging, "stamp.json");
    writeFileSync(stamp, `${JSON.stringify({ version, target }, null, 2)}\n`);
    renameSync(extracted, binary);
    renameSync(stamp, stampPath);
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }
  console.log(`diffr ${version} (${target}) installed at ${binary}`);
}
