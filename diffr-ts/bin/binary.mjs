#!/usr/bin/env node
import { createHash } from "node:crypto";
import { spawnSync } from "node:child_process";
import { chmodSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const targets = { "darwin-arm64": "aarch64-apple-darwin", "darwin-x64": "x86_64-apple-darwin", "linux-x64": "x86_64-unknown-linux-gnu", "linux-arm64": "aarch64-unknown-linux-gnu", "win32-x64": "x86_64-pc-windows-msvc" };

const pending = new Map();
export function ensureBinary(options = {}) {
  const { version } = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const into = options.into ?? join(process.env.XDG_CACHE_HOME || join(homedir(), ".cache"), "diffr", version, `${process.platform}-${process.arch}`, options.full ? "full" : "lean");
  const values = { full: false, required: true, pins: join(root, "pins.json"), ...options, into };
  const key = JSON.stringify([resolve(into), values.full, values.pins, values.check, values.required]);
  if (pending.has(key)) return pending.get(key);
  const task = installBinary(values).finally(() => pending.delete(key));
  pending.set(key, task);
  return task;
}

async function installBinary(values) {
  if (!values.into) throw new Error("--into <dir> is required");
  const into = resolve(values.into);
  const { version } = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const pins = JSON.parse(readFileSync(values.pins, "utf8"));
  if (pins.version !== version) throw new Error(`pins.json is for ${pins.version}, package is ${version}; run \`npm run pin\``);
  const target = targets[`${process.platform}-${process.arch}`];
  if (!target) throw new Error(`no diffr release for ${process.platform}-${process.arch}`);
  const pin = values.full ? pins.full : pins;
  if (!pin) throw new Error(`pins.json has no full edition for ${version}`);
  const expected = pin.sha256?.[target];
  if (typeof expected !== "string" || !/^[a-f0-9]{64}$/.test(expected)) {
    throw new Error(`pins.json has no valid sha256 for ${target}`);
  }

  const name = process.platform === "win32" ? "diffr.exe" : "diffr";
  const binary = join(into, name);
  const stampPath = join(into, "diffr.stamp.json");
  let installed = false;
  try {
    const stamp = JSON.parse(readFileSync(stampPath, "utf8"));
    const stat = lstatSync(binary);
    installed = stat.isFile() && (process.platform === "win32" || (stat.mode & 0o111) !== 0) && stamp.version === version && stamp.target === target
      && Boolean(stamp.full) === Boolean(values.full)
      && typeof stamp.binarySha256 === "string"
      && createHash("sha256").update(readFileSync(binary)).digest("hex") === stamp.binarySha256;
  } catch { /* Missing or damaged installations are fetched again. */ }
  if (installed) {
    return binary;
  }
  if (values.check) throw new Error(`${binary} is missing or not diffr ${version} (${target})`);

  const artifact = values.full ? "diffr-cli-full" : pins.artifact ?? "diffr";
  if (!["diffr", "diffr-cli", "diffr-cli-full"].includes(artifact)) throw new Error(`unknown pinned artifact: ${artifact}`);
  const asset = `${artifact}-${version}-${target}.tar.gz`;
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
    writeFileSync(join(staging, "archive.tar.gz"), bytes);
    // Relative paths: GNU tar on Windows reads "C:" in an archive path as a remote host.
    const tar = spawnSync("tar", ["-xzf", "archive.tar.gz", name], { cwd: staging, encoding: "utf8" });
    const extracted = join(staging, name);
    if (tar.status !== 0 || !lstatSync(extracted).isFile()) {
      throw new Error(`${asset} does not contain a regular ${name} file at its root`);
    }
    chmodSync(extracted, 0o755);
    const stamp = join(staging, "stamp.json");
    writeFileSync(stamp, `${JSON.stringify({ version, target, binarySha256: createHash("sha256").update(readFileSync(extracted)).digest("hex"), ...(values.full && { full: true }) }, null, 2)}\n`);
    renameSync(extracted, binary);
    renameSync(stamp, stampPath);
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }
  return binary;
}
