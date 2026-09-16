import { expect, test } from "bun:test";
import { filterSettings, flattenSchema, fuzzyScore, isDefault, parseValue } from "./config";
/** The shape of `diffr config schema`: `plugins` is inline, one entry per plugin; lists and multi-line
 * values are marked `x-settings: false`. */
export const schemaFixture = {
  $schema: "https://json-schema.org/draft/2020-12/schema",
  title: "Config",
  type: "object",
  properties: {
    plugins: {
      type: "object",
      properties: {
        order: { type: "array", items: { type: "string" }, "x-settings": false, default: ["deleted-bodies", "hide-files", "summarize"] },
        "deleted-bodies": {
          type: "object",
          title: "Collapsed code",
          properties: {
            min_lines: { type: "integer", title: "Shortest body to collapse", "x-group": "Collapsed code", description: "Bodies shorter than this are never summarized or collapsed.", default: 12 },
            enabled: { type: "boolean", title: "Collapse deleted functions", "x-group": "Collapsed code", description: "Collapse deleted function bodies.", default: true },
          },
        },
        "hide-files": {
          type: "object",
          title: "Hidden files",
          properties: {
            enabled: { type: "boolean", title: "Hide test files", "x-group": "Hidden files", description: "Hide files tagged test.", default: true },
            tags: { type: "array", items: { type: "string" }, "x-group": "Hidden files", "x-settings": false, default: ["test"] },
          },
        },
        summarize: {
          type: "object",
          title: "Summaries",
          properties: {
            provider: { type: "string", title: "Provider", "x-group": "Summaries", enum: ["gemini", "none"], description: "Model provider.", default: "gemini" },
            api_key: { title: "API key", "x-group": "Summaries", anyOf: [{ type: "string" }, { type: "null" }], description: "API key for the provider.", default: null },
            model: { $ref: "#/$defs/Model", title: "Model", "x-group": "Summaries" },
            system_prompt: { type: "string", title: "System prompt", "x-group": "Summaries", "x-settings": false, description: "The system instruction.", default: "Summarize." },
          },
        },
      },
    },
  },
  $defs: {
    Model: { type: "string", description: "Model name.", default: "gemini-2.5-flash" },
  },
};
export const valuesFixture = {
  plugins: {
    order: ["deleted-bodies", "hide-files", "summarize"],
    "deleted-bodies": { min_lines: 12, enabled: true },
    "hide-files": { enabled: false, tags: ["test"] },
    summarize: { provider: "gemini", api_key: null, model: "gemini-2.5-flash", system_prompt: "Summarize." },
  },
};
test("schema flattens to dotted keys with descriptions, defaults, and current values", () => {
  const settings = flattenSchema(schemaFixture, valuesFixture);
  expect(settings.map((s) => [s.key, s.type, s.default, s.value])).toEqual([
    ["plugins.deleted-bodies.min_lines", "integer", 12, 12],
    ["plugins.deleted-bodies.enabled", "boolean", true, true],
    ["plugins.hide-files.enabled", "boolean", true, false],
    ["plugins.summarize.provider", "enum", "gemini", "gemini"],
    ["plugins.summarize.api_key", "string", null, null],
    ["plugins.summarize.model", "string", "gemini-2.5-flash", "gemini-2.5-flash"],
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
  const schema = structuredClone(schemaFixture);
  delete (schema.properties.plugins.properties["deleted-bodies"].properties.min_lines as Record<string, unknown>).title;
  expect(() => flattenSchema(schema, valuesFixture)).toThrow("plugins.deleted-bodies.min_lines");
});
test("keys marked x-settings: false are left to the file; any other list or table is a schema error", () => {
  const keys = flattenSchema(schemaFixture, valuesFixture).map((s) => s.key);
  expect(keys.some((key) => key.includes("order") || key.includes("queries") || key.includes("tags"))).toBe(false);
  const schema = structuredClone(schemaFixture);
  delete (schema.properties.plugins.properties["hide-files"].properties.tags as Record<string, unknown>)["x-settings"];
  expect(() => flattenSchema(schema, valuesFixture)).toThrow("Setting plugins.hide-files.tags has unsupported type array");
});
test("fuzzy filtering narrows over title, key, group and description, keeping groups together", () => {
  const settings = flattenSchema(schemaFixture, valuesFixture);
  expect(fuzzyScore("hfenabled", "plugins.hide-files.enabled")).not.toBeNull();
  expect(fuzzyScore("xyz", "plugins.hide-files.enabled")).toBeNull();
  // An empty query keeps schema order, which is already grouped.
  expect(filterSettings(settings, "").map((s) => s.key)).toEqual(settings.map((s) => s.key));
  expect(filterSettings(settings, "hide test").map((s) => s.key)).toEqual(["plugins.hide-files.enabled"]);
  expect(filterSettings(settings, "api")[0].key).toBe("plugins.summarize.api_key");
  // A description mention ("... or collapsed.") still finds the setting.
  expect(filterSettings(settings, "shorter").map((s) => s.key)).toEqual(["plugins.deleted-bodies.min_lines"]);
  // Matching a group name lists the group in schema order.
  expect(filterSettings(settings, "summaries").map((s) => s.key)).toEqual(["plugins.summarize.provider", "plugins.summarize.api_key", "plugins.summarize.model"]);
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
