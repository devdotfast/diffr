/** Paint measured Hunk-style code cells; source identity and viewport geometry stay outside React. */
import { memo } from "react";
import { StyledText, parseColor } from "@opentui/core";
import type {
  RenderSpan,
  SplitLineCell,
  UnifiedLineCell,
} from "./diffRowModel";
import type { Geometry, MeasuredRow } from "../../diffr/geometry";
import type { Palette } from "../../diffr/theme";
import type { RowFold } from "../../diffr/regions";
import { measureTextWidth } from "../lib/text";
const colors = new Map<string, ReturnType<typeof parseColor>>();
function color(value: string) {
  let c = colors.get(value);
  if (!c) {
    c = parseColor(value);
    colors.set(value, c);
  }
  return c;
}
function styled(spans: RenderSpan[], theme: Palette, bg: string) {
  return new StyledText(
    spans.map((span) => ({
      __isChunk: true as const,
      text: span.text,
      fg: color(span.fg ?? theme.fg),
      bg: color(span.bg ?? bg),
    })),
  );
}
/** VS Code's showFoldingControls "always": expandable rows keep their chevron visible. */
function chevron(fold: RowFold | undefined) {
  if (!fold) return " ";
  return fold.collapsed ? "▸" : "▾";
}
/** A multi-line label (pseudocode) hangs under the header, so the header shows only the ellipsis. */
const placeholderText = (fold: RowFold) =>
  fold.label === "" || fold.label.includes("\n") ? " ⋯" : ` ⋯ ${fold.label}`;
export const CodeRowView = memo(function CodeRowView({
  measured,
  visualLine,
  geometry,
  theme,
  selectedSide,
  onSelect,
  onExtend,
  onFold,
}: {
  measured: MeasuredRow;
  visualLine: number;
  geometry: Geometry;
  theme: Palette;
  selectedSide?: "left" | "right";
  onSelect: (side: "left" | "right") => void;
  onExtend: () => void;
  onFold: (fold: RowFold, recursive: boolean) => void;
}) {
  const row = measured.row;
  const lastLine = visualLine === measured.height - 1;
  function cell(
    value: SplitLineCell | UnifiedLineCell,
    spans: RenderSpan[],
    width: number,
    side: "left" | "right",
    unified = false,
  ) {
    const fold = value.fold;
    const bg =
      selectedSide === side
        ? theme.highlight
        : value.foldLabel || fold?.collapsed
          ? theme.foldBackground
          : value.kind === "addition"
            ? theme.addition
            : value.kind === "deletion"
              ? theme.deletion
              : theme.bg;
    // Row colours carry addition and deletion, so the gutter holds numbers and the chevron only.
    const digits = geometry.gutter - 4;
    const number = (n: number | undefined) => `${visualLine ? "" : (n ?? "")}`.padStart(digits);
    const numbers = unified
      ? ` ${number((value as UnifiedLineCell).oldLineNumber)} ${number((value as UnifiedLineCell).newLineNumber)} `
      : ` ${number("lineNumber" in value ? value.lineNumber : undefined)} `;
    const gutterWidth = unified ? geometry.unifiedGutter : geometry.gutter;
    const available = Math.max(1, width - gutterWidth);
    const collapsed = fold?.collapsed && lastLine ? fold : undefined;
    const used = collapsed
      ? Math.min(available, spans.reduce((n, s) => n + measureTextWidth(s.text), 0))
      : available;
    const fill = available - used;
    return (
      <box
        width={width}
        height={1}
        flexDirection="row"
        backgroundColor={bg}
        onMouseDown={(event) => {
          if (event.button === 0) onSelect(side);
        }}
        onMouseMove={onExtend}
      >
        <text width={numbers.length} height={1} fg={theme.muted} selectable={false}>
          {numbers}
        </text>
        <text
          width={1}
          height={1}
          fg={theme.muted}
          selectable={false}
          onMouseDown={(event) => {
            if (fold && !visualLine) event.stopPropagation();
          }}
          onMouseUp={(event) => {
            if (fold && !visualLine && event.button === 0) {
              event.stopPropagation();
              onFold(fold, event.modifiers.alt);
            }
          }}
        >
          {visualLine ? " " : chevron(fold)}
        </text>
        <text width={1} height={1} selectable={false}>
          {" "}
        </text>
        <text
          width={used}
          height={1}
          content={styled(
            selectedSide === side ? spans.map((s) => ({ ...s, bg })) : spans,
            theme,
            bg,
          )}
          selectable={false}
        />
        {collapsed && fill > 0 && (
          <text
            width={fill}
            height={1}
            fg={theme.foldPlaceholder}
            selectable={false}
            onMouseDown={(event) => event.stopPropagation()}
            onMouseUp={(event) => {
              if (event.button === 0) {
                event.stopPropagation();
                onFold(collapsed, event.modifiers.alt);
              }
            }}
          >
            {placeholderText(collapsed)}
          </text>
        )}
      </box>
    );
  }
  return (
    <box height={1} width="100%" flexDirection="row">
      {row.cell ? (
        cell(
          row.cell,
          measured.cell[visualLine] ?? [],
          geometry.leftWidth + geometry.rightWidth + 1,
          row.cell.newLineNumber === undefined ? "left" : "right",
          true,
        )
      ) : (
        <>
          {cell(
            row.left!,
            measured.left[visualLine] ?? [],
            geometry.leftWidth,
            "left",
          )}
          <text width={1} fg={theme.muted} selectable={false}>
            │
          </text>
          {cell(
            row.right!,
            measured.right[visualLine] ?? [],
            geometry.rightWidth,
            "right",
          )}
        </>
      )}
    </box>
  );
});
