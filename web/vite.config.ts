import { existsSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";
import { defineConfig, type Plugin } from "vite";

const repo = fileURLToPath(new URL("..", import.meta.url));
// @pierre/diffs exports neither its stylesheet nor its icon sprite; the diff view draws its own
// rows in that DOM, so it reaches into the package for both.
const diffsDist = fileURLToPath(new URL("node_modules/@pierre/diffs/dist/", import.meta.url));

const engine = fileURLToPath(new URL("src/wasm/diffr_web_bg.wasm", import.meta.url));

/**
 * The engine is 64 MB and static hosts often cap a file at 25 MiB, so the page loads a
 * gzipped copy (7 MB) and unpacks it as it compiles. Written beside the engine whenever it is newer.
 */
function gzipEngine(): Plugin {
  const write = () => {
    if (!existsSync(engine)) return;
    const gz = `${engine}.gz`;
    if (existsSync(gz) && statSync(gz).mtimeMs >= statSync(engine).mtimeMs) return;
    writeFileSync(gz, gzipSync(readFileSync(engine), { level: 9 }));
  };
  return { name: "gzip-engine", buildStart: write, configureServer: write };
}

/**
 * wasm-bindgen's loader names the raw engine as its default source, which would ship it too; the
 * page always hands workers the compiled module instead, so the raw file is never fetched.
 */
function dropRawEngine(): Plugin {
  return {
    name: "drop-raw-engine",
    apply: "build",
    generateBundle(_, bundle) {
      for (const name of Object.keys(bundle)) if (/diffr_web_bg-[\w-]+\.wasm$/.test(name)) delete bundle[name];
    },
  };
}

export default defineConfig({
  plugins: [gzipEngine(), dropRawEngine()],
  // The TUI's diff model (tui/packages/hunk/src/diffr) resolves its packages from here.
  resolve: {
    dedupe: ["zod"],
    alias: {
      "pierre-diffs-style": `${diffsDist}style.js`,
      "pierre-diffs-sprite": `${diffsDist}sprite.js`,
      "geist-fonts": fileURLToPath(new URL("node_modules/geist/dist/fonts", import.meta.url)),
    },
  },
  server: { fs: { allow: [repo] } },
  worker: { format: "es", plugins: () => [dropRawEngine()] },
  build: { target: "es2022" },
});
