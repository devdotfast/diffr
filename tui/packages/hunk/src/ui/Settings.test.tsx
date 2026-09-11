import { expect, test } from "bun:test";
import { testRender } from "@opentui/react/test-utils";
import { act } from "react";
import { Settings, displayValue, isSecret } from "./Settings";
import { schemaFixture, valuesFixture } from "../diffr/config.test";
import type { ConfigClient } from "../diffr/config";
test("empty values show as unset and credential keys are masked", () => {
  expect(displayValue("")).toBe("<unset>");
  expect(displayValue(null)).toBe("<unset>");
  expect(displayValue(12)).toBe("12");
  expect(isSecret("summarize.api_key")).toBe(true);
  expect(isSecret("folds.min_lines")).toBe(false);
});
test("the list filters as you type and the edit view saves booleans, numbers, and enums", async () => {
  const writes: [string, string][] = [];
  const client: ConfigClient = {
    schema: () => schemaFixture,
    show: () => structuredClone(valuesFixture),
    set: (key, value) => { writes.push([key, value]); },
  };
  let quit = false;
  const t = await testRender(<Settings client={client} onQuit={() => { quit = true; }} />, { width: 120, height: 16 });
  const press = async (key: string) => {
    await act(async () => { t.mockInput.pressKey(key); });
    await act(async () => { await t.renderOnce(); });
  };
  const type = async (text: string) => {
    for (const char of text) await press(char);
  };
  const frame = () => t.captureCharFrame();
  // The rows between the search line and the footer.
  const rows = () => frame().split("\n").slice(2).filter((l) => !l.includes("esc quit")).join("\n");
  try {
    await act(async () => { await t.renderOnce(); });
    expect(frame()).toContain("folds.min_lines");
    expect(frame()).toContain("(default: 12)");
    expect(frame()).toContain("↑↓ move · enter edit · type to search · esc quit");
    // api_key is unset; collapse_tests differs from its default and is marked.
    expect(frame().split("\n").find((l) => l.includes("api_key"))).toContain("<unset>");
    expect(frame().split("\n").find((l) => l.includes("collapse_tests"))).toContain("●");
    // Boolean: the edit view lists both choices; y picks true, enter saves.
    await type("cltest");
    expect(rows()).toContain("collapse_tests");
    expect(rows()).not.toContain("min_lines");
    await press("RETURN");
    expect(frame()).toContain("Hide files classed as test.");
    expect(frame()).toContain("default: true");
    expect(frame()).toContain("▸ false");
    expect(frame()).toContain("enter save · esc back");
    await press("y");
    expect(frame()).toContain("▸ true");
    await press("RETURN");
    expect(frame()).toContain("folds.collapse_tests = true");
    expect(writes).toEqual([["folds.collapse_tests", "true"]]);
    expect(frame().split("\n").find((l) => l.includes("collapse_tests"))).not.toContain("●");
    // Number: a text field, esc discards, enter saves.
    for (let i = 0; i < 6; i++) await press("BACKSPACE");
    await type("min");
    await press("RETURN");
    expect(frame()).toContain("value: 12▏");
    await press("BACKSPACE");
    await type("9");
    await press("ESCAPE");
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 100)); });
    await act(async () => { await t.renderOnce(); });
    expect(frame()).toContain("↑↓ move");
    expect(writes).toHaveLength(1);
    await press("RETURN");
    await press("BACKSPACE");
    await press("BACKSPACE");
    await type("20");
    await press("RETURN");
    expect(frame()).toContain("folds.min_lines = 20");
    expect(writes.at(-1)).toEqual(["folds.min_lines", "20"]);
    // Enum: arrows move through the options.
    for (let i = 0; i < 3; i++) await press("BACKSPACE");
    await type("provider");
    await press("RETURN");
    expect(frame()).toContain("▸ gemini");
    await act(async () => { t.mockInput.pressArrow("down"); });
    await act(async () => { await t.renderOnce(); });
    expect(frame()).toContain("▸ none");
    await press("RETURN");
    expect(writes.at(-1)).toEqual(["summarize.provider", "none"]);
    // Secret: typed masked.
    for (let i = 0; i < 8; i++) await press("BACKSPACE");
    await type("api_key");
    await press("RETURN");
    await type("abc");
    expect(frame()).toContain("value: •••▏");
    expect(frame()).not.toContain("abc");
    await press("RETURN");
    expect(writes.at(-1)).toEqual(["summarize.api_key", "abc"]);
    expect(rows().split("\n").find((l) => l.includes("api_key"))).toContain("••••••");
    expect(frame()).not.toContain("abc");
    // A lone escape is only recognised once the parser's escape-sequence timeout passes.
    await press("ESCAPE");
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 100)); });
    expect(quit).toBe(true);
  } finally {
    await act(async () => { t.renderer.destroy(); });
  }
});
