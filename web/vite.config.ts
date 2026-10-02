import { fileURLToPath } from "node:url";
import { defineConfig } from "vite";

const repo = fileURLToPath(new URL("..", import.meta.url));
// @pierre/diffs exports neither its stylesheet nor its icon sprite; the diff view draws its own
// rows in that DOM, so it reaches into the package for both.
const diffsDist = fileURLToPath(new URL("node_modules/@pierre/diffs/dist/", import.meta.url));

export default defineConfig({
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
  worker: { format: "es" },
  build: { target: "es2022" },
});
