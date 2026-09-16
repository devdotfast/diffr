import { expect, test } from "bun:test";
import { testRender } from "@opentui/react/test-utils";
import { act } from "react";
import { Settings, displayValue, isSecret, nextValue } from "./Settings";
import { schemaFixture, valuesFixture } from "../diffr/config.test";
import { flattenSchema, type ConfigClient } from "../diffr/config";

test("values show as not set, secrets only as stored, and toggles flip or cycle", () => {
  const [minLines, collapse, , provider, apiKey] = flattenSchema(schemaFixture, valuesFixture);
  expect(displayValue(minLines)).toBe("12");
  expect(displayValue({ ...minLines, value: null })).toBe("not set");
  expect(displayValue(apiKey)).toBe("not set");
  expect(displayValue({ ...apiKey, value: "abc" })).toBe("✓ stored");
  expect(isSecret("plugins.summarize.api_key")).toBe(true);
  expect(isSecret("plugins.deleted-bodies.min_lines")).toBe(false);
  expect(nextValue(collapse)).toBe("false");
  expect(nextValue(provider)).toBe("none");
  expect(nextValue({ ...provider, value: "none" })).toBe("gemini");
});

test("rows lead with titles under group headings; toggles change in place and typed values open a prompt", async () => {
  const writes: [string, string][] = [];
  const client: ConfigClient = {
    schema: () => schemaFixture,
    show: () => structuredClone(valuesFixture),
    set: (key, value) => {
      writes.push([key, value]);
    },
  };
  let quit = false;
  const t = await testRender(
    <Settings client={client} initial={flattenSchema(schemaFixture, valuesFixture)} onQuit={() => { quit = true; }} />,
    { width: 100, height: 24 },
  );
  const press = async (key: string) => {
    await act(async () => { t.mockInput.pressKey(key); });
    await act(async () => { await t.renderOnce(); });
  };
  const type = async (text: string) => {
    for (const char of text) await press(char);
  };
  const clear = async () => {
    for (let i = 0; i < 12; i++) await press("BACKSPACE");
  };
  const escape = async () => {
    await press("ESCAPE");
    // A lone escape is only recognised once the parser's escape-sequence timeout passes.
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 100)); });
    await act(async () => { await t.renderOnce(); });
  };
  const frame = () => t.captureCharFrame();
  const line = (text: string) => frame().split("\n").find((l) => l.includes(text)) ?? "";
  try {
    await act(async () => { await t.renderOnce(); });
    // Groups in schema order, each setting under its heading, titles first and keys only in the detail line.
    const order = ["Collapsed code", "Shortest body to collapse", "Collapse deleted functions", "Hidden files", "Hide test files", "Summaries", "Provider"]
      .map((text) => frame().split("\n").findIndex((l) => l.includes(text)));
    expect(order).toEqual([...order].sort((a, b) => a - b));
    expect(order.every((index) => index >= 0)).toBe(true);
    expect(line("Shortest body to collapse")).toContain("→");
    expect(line("Shortest body to collapse")).not.toContain("plugins.deleted-bodies.min_lines");
    expect(frame()).toContain("plugins.deleted-bodies.min_lines · default 12");
    expect(frame()).toContain("Bodies shorter than this are never summarized or collapsed.");
    expect(frame()).toContain("(1/6)");
    expect(frame()).toContain("Type to search · Enter/Space to change · Esc to quit");
    expect(line("API key")).toContain("not set");

    // Boolean: space flips it in place and writes through the CLI.
    await type("hidetest");
    expect(frame()).toContain("(1/1)");
    expect(line("Hide test files")).toContain("false");
    await press(" ");
    expect(writes).toEqual([["plugins.hide-files.enabled", "true"]]);
    expect(line("Hide test files")).toContain("true");
    expect(frame()).toContain("Hide test files: true");

    // Enum: enter cycles to the next option.
    await clear();
    await type("provider");
    await press("RETURN");
    expect(writes.at(-1)).toEqual(["plugins.summarize.provider", "none"]);
    expect(line("Provider")).toContain("none");

    // Number: enter opens a prompt titled by the setting; escape discards, enter saves.
    await clear();
    await type("shortest");
    await press("RETURN");
    expect(frame()).toContain("Shortest body to collapse");
    expect(frame()).toContain("> 12");
    expect(frame()).toContain("(escape/ctrl+c to cancel, enter to submit)");
    await press("BACKSPACE");
    await type("9");
    await escape();
    expect(frame()).toContain("Type to search");
    expect(writes).toHaveLength(2);
    await press("RETURN");
    await press("BACKSPACE");
    await press("BACKSPACE");
    await type("2.5");
    await press("RETURN");
    expect(frame()).toContain("Expected an integer");
    expect(writes).toHaveLength(2);
    await press("BACKSPACE");
    await press("BACKSPACE");
    await press("BACKSPACE");
    await type("20");
    await press("RETURN");
    expect(writes.at(-1)).toEqual(["plugins.deleted-bodies.min_lines", "20"]);
    expect(line("Shortest body to collapse")).toContain("20");

    // Secret: the prompt starts empty and masks typing; the row only says it is stored.
    await clear();
    await type("api");
    await press("RETURN");
    expect(frame()).toContain("API key for the provider.");
    // Keys that arrive in one burst, before any re-render, all land, including the enter after them.
    await act(async () => {
      for (const key of ["a", "b", "c"]) t.mockInput.pressKey(key);
    });
    await act(async () => { await t.renderOnce(); });
    expect(frame()).toContain("> •••");
    expect(frame()).not.toContain("abc");
    await act(async () => {
      t.mockInput.pressKey("d");
      t.mockInput.pressKey("RETURN");
    });
    await act(async () => { await t.renderOnce(); });
    expect(writes.at(-1)).toEqual(["plugins.summarize.api_key", "abcd"]);
    expect(line("API key")).toContain("✓ stored");
    expect(frame()).not.toContain("abc");

    await escape();
    expect(quit).toBe(true);
  } finally {
    await act(async () => { t.renderer.destroy(); });
  }
});
