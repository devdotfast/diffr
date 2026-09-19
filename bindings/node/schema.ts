// Shared validation for the serialized search contract; no native binding import.
import { z } from "zod";
import type { RegionData, SearchResultData } from "./types.ts";

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
function pairing<T extends z.ZodType>(value: T) {
  return z.union([
    z.strictObject({ lhs: value, rhs: value }),
    z.strictObject({ lhs: value }),
    z.strictObject({ rhs: value }),
  ]);
}
const worktree = z.strictObject({ commitId: z.string(), path: z.string() });
export const searchResultDataSchema: z.ZodType<SearchResultData> = z.strictObject({
  kind: z.enum(["combined", "lhs", "rhs", "unchanged"]),
  scope: z.strictObject({ repo: z.string(), baseWorktree: worktree, headWorktree: worktree }),
  file: pairing(fileRef), sources: pairing(source),
});
export type { RegionData, SourceData, SearchResultData } from "./types.ts";
