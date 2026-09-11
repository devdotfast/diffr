import { expect, test } from "bun:test";
import { testRender } from "@opentui/react/test-utils";
import { act } from "react";
import { Settings } from "./Settings";
import { schemaFixture, valuesFixture } from "../diffr/config.test";
import type { ConfigClient } from "../diffr/config";
test("settings list filters as you type, toggles booleans, and edits numbers through the client", async () => {
  const writes: [string, string][] = [];
  const client: ConfigClient = {
    schema: () => schemaFixture,
    show: () => structuredClone(valuesFixture),
    set: (key, value) => { writes.push([key, value]); },
  };
  let quit = false;
  const t = await testRender(<Settings client={client} onQuit={() => { quit = true; }} />, { width: 120, height: 14 });
  const press = async (key: string) => {
    await act(async () => { t.mockInput.pressKey(key); });
    await act(async () => { await t.renderOnce(); });
  };
  const type = async (text: string) => {
    for (const char of text) await press(char);
  };
  // The rows between the search line and the footer.
  const rows = (frame: string) => frame.split("\n").slice(2).filter((l) => !l.includes("esc:")).join("\n");
  try {
    await act(async () => { await t.renderOnce(); });
    const frame = () => t.captureCharFrame();
    expect(frame()).toContain("folds.min_lines");
    expect(frame()).toContain("(default: 12)");
    // collapse_tests differs from its default and is marked.
    expect(frame().split("\n").find((l) => l.includes("collapse_tests"))).toContain("●");
    await type("cltest");
    expect(rows(frame())).toContain("collapse_tests");
    expect(rows(frame())).not.toContain("min_lines");
    await press("RETURN");
    expect(frame()).toContain("folds.collapse_tests = true");
    expect(writes).toEqual([["folds.collapse_tests", "true"]]);
    for (let i = 0; i < 6; i++) await press("BACKSPACE");
    await type("min");
    expect(rows(frame())).toContain("min_lines");
    expect(rows(frame())).not.toContain("collapse_tests");
    await press("RETURN");
    expect(frame()).toContain("enter: save");
    await press("BACKSPACE");
    await press("BACKSPACE");
    await type("20");
    await press("RETURN");
    expect(frame()).toContain("folds.min_lines = 20");
    expect(writes.at(-1)).toEqual(["folds.min_lines", "20"]);
    // A lone escape is only recognised once the parser's escape-sequence timeout passes.
    await press("ESCAPE");
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 100)); });
    expect(quit).toBe(true);
  } finally {
    await act(async () => { t.renderer.destroy(); });
  }
});
