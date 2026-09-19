// Shared validation for the serialized search contract; no native binding import.
import { z } from "zod";
import type { RegionData, SearchResultData, SourceData } from "./types.ts";

const coordinate = z.number().int().nonnegative();
const position = z.strictObject({ line: coordinate, column: coordinate });
const span = z.strictObject({ line: coordinate, start_column: coordinate, end_column: coordinate });
const regionFields = {
  id: coordinate,
  fold_state_id: coordinate,
  start: position,
  end: position,
  tags: z.array(z.string()).optional(),
  visibility: z.strictObject({ collapsed: z.boolean().optional(), label: z.string().optional() }).optional(),
};
export const regionDataSchema: z.ZodType<RegionData> = z.lazy(() => z.discriminatedUnion("kind", [
  z.strictObject({ ...regionFields, kind: z.literal("leaf"), alignment_id: coordinate,
    changed: z.array(span).optional(), search_highlights: z.array(span).optional() }),
  z.strictObject({ ...regionFields, kind: z.literal("fold"), children: z.array(regionDataSchema) }),
]));
const fileRef = z.strictObject({ path: z.string(), oid: z.string(), mode: z.string() });
const source = z.strictObject({ text: z.string(),
  syntax: z.array(span.extend({ capture: z.string() })).optional(), regions: z.array(regionDataSchema) });
const worktree = z.strictObject({ commitId: z.string(), path: z.string() });
const common = {
  display: z.enum(["both", "lhs", "rhs"]),
  scope: z.strictObject({ repo: z.string(), baseWorktree: worktree, headWorktree: worktree }),
};
export const searchResultDataSchema: z.ZodType<SearchResultData> = z.union([
  z.strictObject({ ...common, file: z.strictObject({lhs: fileRef, rhs: fileRef}), sources: z.strictObject({same: source}) }),
  z.strictObject({ ...common, file: z.strictObject({lhs: fileRef, rhs: fileRef}), sources: z.strictObject({lhs: source, rhs: source}) }),
  z.strictObject({ ...common, file: z.strictObject({lhs: fileRef}), sources: z.strictObject({lhs: source}) }),
  z.strictObject({ ...common, file: z.strictObject({rhs: fileRef}), sources: z.strictObject({rhs: source}) }),
]).superRefine((result, context) => {
  const fail = (message: string) => context.addIssue({code: "custom", message});
  if (result.display !== "both" && !(result.display in result.file)) fail("Display requests an absent side");
  if ("same" in result.sources && "lhs" in result.file && "rhs" in result.file && result.file.lhs.oid !== result.file.rhs.oid) fail("Shared content requires identical blob identities");
  for (const file of Object.values(result.file)) {
    if (!file.path || file.path.startsWith("/") || file.path.includes("\\") || file.path.split("/").some(part => part === ".." || part === ".") || /[\x00-\x1f]/.test(file.path)) fail("File paths must be repository-relative");
    if (!/^(?:[a-f0-9]{40}|[a-f0-9]{64})$/.test(file.oid) || !["100644", "100755"].includes(file.mode)) fail("Expected regular Git blob identity");
  }
  const compare = (a: {line: number; column: number}, b: {line: number; column: number}) => a.line - b.line || a.column - b.column;
  for (const source of Object.values(result.sources) as SourceData[]) {
    const lines = source.text.split("\n");
    const bytes = lines.map(line => {
      const boundaries = new Set([0]); let length = 0;
      for (const char of line) { length += new TextEncoder().encode(char).length; boundaries.add(length); }
      return boundaries;
    });
    const position = (pos: {line: number; column: number}) => bytes[pos.line]?.has(pos.column) === true;
    const walk = (regions: RegionData[], parent?: RegionData) => {
      for (const region of regions) {
        if (!position(region.start) || !position(region.end) || compare(region.start, region.end) > 0) fail("Region is outside source bounds");
        if (parent && (compare(region.start, parent.start) < 0 || compare(region.end, parent.end) > 0)) fail("Child region is outside its parent");
        if (region.kind === "fold") walk(region.children, region);
        else for (const span of [...(region.changed ?? []), ...(region.search_highlights ?? [])]) {
          const start = {line: span.line, column: span.start_column}, end = {line: span.line, column: span.end_column};
          if (!position(start) || !position(end) || compare(start, end) > 0 || compare(start, region.start) < 0 || compare(end, region.end) > 0) fail("Highlight is outside its leaf or splits a UTF-8 character");
        }
      }
    };
    walk(source.regions);
    for (const span of source.syntax ?? []) if (!position({line: span.line, column: span.start_column}) || !position({line: span.line, column: span.end_column}) || span.start_column > span.end_column) fail("Syntax span is outside source bounds");
  }
});
export type { RegionData, SourceData, SearchResultData } from "./types.ts";
