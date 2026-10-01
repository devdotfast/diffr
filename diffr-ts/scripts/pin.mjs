#!/usr/bin/env node
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");
const version = JSON.parse(readFileSync(join(root, "package.json"), "utf8")).version;
const targets = ["aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "x86_64-pc-windows-msvc"];
async function hashes(artifact) {
  const sha256 = {};
  for (const target of targets) {
    const url = `https://github.com/devdotfast/diffr/releases/download/${version}/${artifact}-${version}-${target}.tar.gz`;
    const response = await fetch(url, { redirect: "follow", signal: AbortSignal.timeout(120_000) });
    if (!response.ok) {
      console.error(`pin: GET ${url} failed with ${response.status} ${response.statusText}`);
      process.exit(1);
    }
    sha256[target] = createHash("sha256").update(Buffer.from(await response.arrayBuffer())).digest("hex");
    console.log(`${artifact} ${target} ${sha256[target]}`);
  }
  return sha256;
}
const pins = { version, artifact: "diffr-cli", sha256: await hashes("diffr-cli"), full: { sha256: await hashes("diffr-cli-full") } };
writeFileSync(join(root, "pins.json"), `${JSON.stringify(pins, null, 2)}\n`);
console.log(`wrote pins.json for ${version}`);
