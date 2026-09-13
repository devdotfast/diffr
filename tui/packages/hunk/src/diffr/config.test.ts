import { expect, test } from "bun:test";
import { filterSettings, flattenSchema, fuzzyScore, isDefault, parseValue } from "./config";
export const schemaFixture = {
  $schema: "https://json-schema.org/draft/2020-12/schema",
  title: "Params",
  type: "object",
  properties: {
    folds: {
      type: "object",
      properties: {
        min_lines: { type: "integer", title: "Shortest body to collapse", "x-group": "Collapsed code", description: "Bodies shorter than this are never summarized or collapsed.", default: 12 },
        collapse_deleted: { type: "boolean", title: "Collapse deleted functions", "x-group": "Collapsed code", description: "Collapse deleted function bodies.", default: true },
        collapse_tests: { type: "boolean", title: "Hide test files", "x-group": "Hidden files", description: "Hide files classed as test.", default: true },
      },
    },
    summarize: {
      type: "object",
      properties: {
        provider: { type: "string", title: "Provider", "x-group": "Summaries", enum: ["gemini", "none"], description: "Model provider.", default: "gemini" },
        api_key: { title: "API key", "x-group": "Summaries", anyOf: [{ type: "string" }, { type: "null" }], description: "API key for the provider.", default: null },
        model: { $ref: "#/$defs/Model", title: "Model", "x-group": "Summaries" },
      },
    },
  },
  $defs: { Model: { type: "string", description: "Model name.", default: "gemini-2.5-flash" } },
};
export const valuesFixture = {
  folds: { min_lines: 12, collapse_deleted: true, collapse_tests: false },
  summarize: { provider: "gemini", api_key: null, model: "gemini-2.5-flash" },
};
test("schema flattens to dotted keys with descriptions, defaults, and current values", () => {
  const settings = flattenSchema(schemaFixture, valuesFixture);
  expect(settings.map((s) => [s.key, s.type, s.default, s.value])).toEqual([
    ["folds.min_lines", "integer", 12, 12],
    ["folds.collapse_deleted", "boolean", true, true],
    ["folds.collapse_tests", "boolean", true, false],
    ["summarize.provider", "enum", "gemini", "gemini"],
    ["summarize.api_key", "string", null, null],
    ["summarize.model", "string", "gemini-2.5-flash", "gemini-2.5-flash"],
  ]);
  expect(settings.map((s) => [s.title, s.group])).toEqual([
    ["Shortest body to collapse", "Collapsed code"],
    ["Collapse deleted functions", "Collapsed code"],
    ["Hide test files", "Hidden files"],
    ["Provider", "Summaries"],
    ["API key", "Summaries"],
    ["Model", "Summaries"],
  ]);
  expect(settings[3].options).toEqual(["gemini", "none"]);
  expect(settings.map(isDefault)).toEqual([true, true, false, true, true, true]);
});
test("a setting without a title or group is a schema error", () => {
  const schema = structuredClone(schemaFixture) as typeof schemaFixture & Record<string, unknown>;
  delete (schema.properties.folds.properties.min_lines as Record<string, unknown>).title;
  expect(() => flattenSchema(schema, valuesFixture)).toThrow("folds.min_lines");
});
test("fuzzy filtering narrows over title, key, group and description, keeping groups together", () => {
  const settings = flattenSchema(schemaFixture, valuesFixture);
  expect(fuzzyScore("cltest", "folds.collapse_tests")).not.toBeNull();
  expect(fuzzyScore("xyz", "folds.collapse_tests")).toBeNull();
  // An empty query keeps schema order, which is already grouped.
  expect(filterSettings(settings, "").map((s) => s.key)).toEqual(settings.map((s) => s.key));
  expect(filterSettings(settings, "hide test").map((s) => s.key)).toEqual(["folds.collapse_tests"]);
  expect(filterSettings(settings, "api")[0].key).toBe("summarize.api_key");
  // A description mention ("... or collapsed.") still finds the setting.
  expect(filterSettings(settings, "shorter").map((s) => s.key)).toEqual(["folds.min_lines"]);
  // Matching a group name lists the group in schema order.
  expect(filterSettings(settings, "summaries").map((s) => s.key)).toEqual(["summarize.provider", "summarize.api_key", "summarize.model"]);
});
test("edited values are parsed in the setting's type and bad input is rejected", () => {
  const [minLines, collapse, , provider] = flattenSchema(schemaFixture, valuesFixture);
  expect(parseValue(minLines, "20")).toBe(20);
  expect(() => parseValue(minLines, "2.5")).toThrow("integer");
  expect(parseValue(collapse, "false")).toBe(false);
  expect(() => parseValue(collapse, "yes")).toThrow();
  expect(parseValue(provider, "none")).toBe("none");
  expect(() => parseValue(provider, "openai")).toThrow("gemini, none");
});
