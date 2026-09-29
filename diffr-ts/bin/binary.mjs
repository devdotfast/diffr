#!/usr/bin/env node
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { chmodSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const targets = { "darwin-arm64": "aarch64-apple-darwin", "darwin-x64": "x86_64-apple-darwin", "linux-x64": "x86_64-unknown-linux-gnu", "linux-arm64": "aarch64-unknown-linux-gnu" };

/** Download a pinned edition into a versioned cache and return its executable. */
export async function ensureBinary({ directory, edition = "lean", check = false, pins: pinsPath = join(root, "pins.json") }) {
  if (typeof directory !== "string" || !directory) throw new Error("directory is required");
  if (!["lean", "full"].includes(edition)) throw new Error(`unknown diffr edition: ${edition}`);
  const { version } = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const target = targets[`${process.platform}-${process.arch}`];
  if (!target) throw new Error(`no diffr release for ${process.platform}-${process.arch}`);
  return installBinary({ into: join(directory, version, target, edition), edition, check, pinsPath, required: true });
}

export async function installBinary({ into, edition = "lean", check = false, required = false, pinsPath = join(root, "pins.json") }) {
  into = resolve(into);
  const { version } = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const pins = JSON.parse(readFileSync(pinsPath, "utf8"));
  if (pins.version !== version) throw new Error(`pins.json is for ${pins.version}, package is ${version}; run \`npm run pin\``);
  const target = targets[`${process.platform}-${process.arch}`];
  if (!target) throw new Error(`no diffr release for ${process.platform}-${process.arch}`);
  if (!["lean", "full"].includes(edition)) throw new Error(`unknown diffr edition: ${edition}`);
  const pin = pins.editions?.[edition] ?? (edition === "lean" ? pins : undefined);
  if (!pin) throw new Error(`pins.json has no ${edition} edition for ${version}`);
  const expected = pin.sha256?.[target];
  if (typeof expected !== "string" || !/^[a-f0-9]{64}$/.test(expected)) {
    throw new Error(`pins.json has no valid sha256 for ${target}`);
  }

  const artifact = pin.artifact ?? "diffr";
  const allowed = edition === "full" ? ["diffr-cli-full"] : ["diffr", "diffr-cli"];
  if (!allowed.includes(artifact)) throw new Error(`unknown pinned artifact: ${artifact}`);
  const binary = join(into, "diffr");
  const stampPath = join(into, "diffr.stamp.json");
  let installed = false;
  try {
    const stamp = JSON.parse(readFileSync(stampPath, "utf8"));
    const stat = lstatSync(binary);
    installed = stat.isFile() && (stat.mode & 0o111) !== 0 && stamp.version === version && stamp.target === target
      && stamp.edition === edition && stamp.archiveSha256 === expected
      && stamp.binarySha256 === createHash("sha256").update(readFileSync(binary)).digest("hex");
  } catch { /* Missing or damaged installations are fetched again. */ }
  if (installed) {
    console.error(`diffr ${version} (${target}) already at ${binary}`);
    return binary;
  }
  if (check) throw new Error(`${binary} is missing or not diffr ${version} (${target})`);

  const asset = `${artifact}-${version}-${target}.tar.gz`;
  const url = `https://github.com/devdotfast/diffr/releases/download/${version}/${asset}`;
  let bytes;
  try {
    const response = await fetch(url, { redirect: "follow", signal: AbortSignal.timeout(120_000) });
    if (!response.ok) throw new Error(`HTTP ${response.status} ${response.statusText}`);
    bytes = Buffer.from(await response.arrayBuffer());
  } catch (error) {
    if (required) throw new Error(`could not download ${url}: ${error.message}`);
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
    writeFileSync(stamp, `${JSON.stringify({ version, target, edition, archiveSha256: expected, binarySha256: createHash("sha256").update(readFileSync(extracted)).digest("hex") }, null, 2)}\n`);
    renameSync(extracted, binary);
    renameSync(stamp, stampPath);
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }
  console.error(`diffr ${version} (${target}) installed at ${binary}`);
  return binary;
}
