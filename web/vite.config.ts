import { fileURLToPath } from "node:url";
import { defineConfig, type Plugin } from "vite";

const repo = fileURLToPath(new URL("..", import.meta.url));
const hunk = fileURLToPath(new URL("../tui/packages/hunk/src/", import.meta.url));
const textWidth = fileURLToPath(new URL("src/textWidth.ts", import.meta.url));

/**
 * The TUI's diff model (regions, rows, themes) runs here unchanged, bar what Bun gives it:
 * TOML themes imported as text and parsed by Bun.TOML, and terminal text measurement.
 */
function tuiModel(): Plugin {
  return {
    name: "diffr-tui-model",
    enforce: "pre",
    resolveId(source, importer) {
      if (importer?.startsWith(hunk) && source === "../ui/lib/text") return textWidth;
    },
    transform(code, id) {
      if (id !== `${hunk}diffr/theme.ts`) return;
      return `import { parse as __parseToml } from "smol-toml";\n${code}`
        .replace(`import { readFileSync } from "node:fs";`,
          `const readFileSync = (_path: string, _encoding: string): string => { throw new Error("theme files need the native diffr"); };`)
        .replaceAll(`.toml" with { type: "text" }`, `.toml?raw"`)
        .replaceAll("Bun.TOML.parse", "__parseToml");
    },
  };
}

export default defineConfig({
  plugins: [tuiModel()],
  // The TUI's sources resolve their packages from here.
  resolve: { dedupe: ["zod", "string-width", "smol-toml"] },
  server: { fs: { allow: [repo] } },
  worker: { format: "es" },
  build: { target: "es2022" },
});
