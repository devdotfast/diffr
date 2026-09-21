#!/usr/bin/env node
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const root = join(here, "..");
const version = JSON.parse(readFileSync(join(root, "package.json"), "utf8")).version;
const targets = ["aarch64-apple-darwin", "x86_64-unknown-linux-gnu"];
const sha256 = {};
for (const target of targets) {
  const url = `https://github.com/devdotfast/diffr/releases/download/${version}/diffr-${version}-${target}.tar.gz`;
  const response = await fetch(url, { redirect: "follow", signal: AbortSignal.timeout(120_000) });
  if (!response.ok) {
    console.error(`pin: GET ${url} failed with ${response.status} ${response.statusText}`);
    process.exit(1);
  }
  sha256[target] = createHash("sha256").update(Buffer.from(await response.arrayBuffer())).digest("hex");
  console.log(`${target} ${sha256[target]}`);
}
writeFileSync(join(root, "pins.json"), `${JSON.stringify({ version, sha256 }, null, 2)}\n`);
console.log(`wrote pins.json for ${version}`);
