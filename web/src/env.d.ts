/// <reference types="vite/client" />
// The TUI's theme loader is written for Bun; vite.config.ts adapts it for the browser.
declare module "*.toml" {
  const text: string;
  export default text;
}
declare const Bun: { TOML: { parse(text: string): unknown } };
declare module "node:fs" {
  export function readFileSync(path: string, encoding: string): string;
}
