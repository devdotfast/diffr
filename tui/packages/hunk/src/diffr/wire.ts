/** Validate the lossless Rust wire format without translating it through patch metadata. */
import { z } from "zod";
const uint = z.number().int().nonnegative();
const point = z.object({ line: uint, byte_column: uint });
const range = z.object({ start: point, end: point });
const span = z.object({ line: uint, start_col: uint, end_col: uint });
const highlight = z.union([
  z.literal("Delimiter"),
  z.object({
    Atom: z.union([
      z.enum([
        "Normal",
        "Type",
        "Keyword",
        "Comment",
        "TreeSitterError",
        "CanIgnore",
      ]),
      z.object({ String: z.enum(["StringLiteral", "Text"]) }),
    ]),
  }),
]);
const position = z.object({
  pos: span,
  kind: z.union([
    z.object({
      UnchangedToken: z.object({
        highlight,
        self_pos: z.array(span),
        opposite_pos: z.array(span),
      }),
    }),
    z.object({
      UnchangedPartOfNovelItem: z.object({
        highlight,
        self_pos: span,
        opposite_pos: z.array(span),
      }),
    }),
    z.object({ Novel: z.object({ highlight }) }),
    z.object({ NovelWord: z.object({ highlight }) }),
    z.object({ Ignored: z.object({ highlight }) }),
  ]),
});
const fold = z.object({
  tags: z.array(z.string()),
  range,
  match_kind: z.union([
    z.literal("Novel"),
    z.object({ Unchanged: z.object({ opposite: range }) }),
  ]),
  placeholder: z.string(),
});
const source = z.union([z.literal("Binary"), z.object({ Text: z.string() })]);
export const diffResultSchema = z.object({
  display_path: z.string(),
  extra_info: z.string().nullable(),
  file_format: z.union([
    z.enum(["PlainText", "Binary"]),
    z.object({ SupportedLanguage: z.string() }),
    z.object({ TextFallback: z.object({ reason: z.string() }) }),
  ]),
  lhs_src: source,
  rhs_src: source,
  lhs_positions: z.array(position),
  rhs_positions: z.array(position),
  lhs_folds: z.array(fold),
  rhs_folds: z.array(fold),
  hunks: z.array(
    z.object({
      novel_lhs: z.array(uint),
      novel_rhs: z.array(uint),
      lines: z.array(z.tuple([uint.nullable(), uint.nullable()])),
    }),
  ),
  has_byte_changes: z.tuple([uint, uint]).nullable(),
  has_syntactic_changes: z.boolean(),
});
const operand = z.discriminatedUnion("kind", [
  z.object({ kind: z.literal("file"), path: z.string() }),
  z.object({ kind: z.literal("revision"), ref: z.string() }),
  z.object({ kind: z.literal("index") }),
  z.object({ kind: z.literal("working_tree") }),
  z.object({ kind: z.literal("empty_tree") }),
]);
const file = z.object({
  old_path: z.string().nullable(),
  new_path: z.string().nullable(),
  status: z.enum([
    "added",
    "deleted",
    "modified",
    "renamed",
    "type_changed",
    "conflicted",
  ]),
  class: z.string().nullable(),
});
export const eventSchema = z.discriminatedUnion("type", [
  z.object({
    type: z.literal("start"),
    version: z.literal(1),
    before: operand,
    after: operand,
    total: uint,
  }),
  z.object({ type: z.literal("file"), file, diff: diffResultSchema }),
  z.object({ type: z.literal("file_error"), file, message: z.string() }),
  z.object({ type: z.literal("complete"), succeeded: uint, failed: uint }),
]);
export type DiffResult = z.infer<typeof diffResultSchema>;
export type DiffEvent = z.infer<typeof eventSchema>;
export type DiffFile = Extract<DiffEvent, { type: "file" }>;
export type MatchedPos = DiffResult["lhs_positions"][number];
export type Highlight = z.infer<typeof highlight>;
