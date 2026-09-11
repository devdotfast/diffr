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
        min_lines: { type: "integer", description: "Bodies shorter than this are never summarized or collapsed.", default: 12 },
        collapse_deleted: { type: "boolean", description: "Collapse deleted function bodies.", default: true },
        collapse_tests: { type: "boolean", description: "Hide files classed as test.", default: true },
      },
    },
    summarize: {
      type: "object",
      properties: {
        provider: { type: "string", enum: ["gemini", "none"], description: "Model provider.", default: "gemini" },
        api_key: { anyOf: [{ type: "string" }, { type: "null" }], description: "API key for the provider.", default: null },
        model: { $ref: "#/$defs/Model" },
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
  expect(settings[3].options).toEqual(["gemini", "none"]);
  expect(settings.map(isDefault)).toEqual([true, true, false, true, true, true]);
});
test("fuzzy filtering narrows over key and description", () => {
  const settings = flattenSchema(schemaFixture, valuesFixture);
  expect(fuzzyScore("cltest", "folds.collapse_tests")).not.toBeNull();
  expect(fuzzyScore("xyz", "folds.collapse_tests")).toBeNull();
  // Key matches rank above a description mention ("... or collapsed.").
  expect(filterSettings(settings, "coll").map((s) => s.key)).toEqual(["folds.collapse_deleted", "folds.collapse_tests", "folds.min_lines"]);
  expect(filterSettings(settings, "shorter").map((s) => s.key)).toEqual(["folds.min_lines"]);
  expect(filterSettings(settings, "")).toHaveLength(6);
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
