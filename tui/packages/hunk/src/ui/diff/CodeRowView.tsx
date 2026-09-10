/** Paint measured Hunk-style code cells; source identity and viewport geometry stay outside React. */
import { memo } from "react";
import { StyledText, parseColor } from "@opentui/core";
import type {
  RenderSpan,
  SplitLineCell,
  UnifiedLineCell,
} from "./diffRowModel";
import type { Geometry, MeasuredRow } from "../../diffr/geometry";
import type { Palette } from "../../diffr/rows";
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
export const CodeRowView = memo(function CodeRowView({
  measured,
  visualLine,
  geometry,
  theme,
  selectedSide,
  onSelect,
  onExtend,
}: {
  measured: MeasuredRow;
  visualLine: number;
  geometry: Geometry;
  theme: Palette;
  selectedSide?: "left" | "right";
  onSelect: (side: "left" | "right") => void;
  onExtend: () => void;
}) {
  const row = measured.row;
  function cell(
    value: SplitLineCell | UnifiedLineCell,
    spans: RenderSpan[],
    width: number,
    side: "left" | "right",
    unified = false,
  ) {
    const bg =
      selectedSide === side
        ? "#264f78"
        : value.kind === "addition"
          ? theme.addition
          : value.kind === "deletion"
            ? theme.deletion
            : theme.bg;
    const number = "lineNumber" in value ? value.lineNumber : undefined;
    const gutter = unified
      ? `${visualLine ? "" : ((value as UnifiedLineCell).oldLineNumber ?? "")}`.padStart(
          geometry.gutter - 1,
        ) +
        " " +
        `${visualLine ? "" : ((value as UnifiedLineCell).newLineNumber ?? "")}`.padStart(
          geometry.gutter - 2,
        ) +
        (visualLine ? " " : value.sign) +
        " "
      : `${visualLine ? "" : (number ?? "")}`.padStart(geometry.gutter - 2) +
        (visualLine ? " " : value.sign) +
        " ";
    const gutterWidth = unified ? geometry.gutter * 2 : geometry.gutter;
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
        <text
          width={gutterWidth}
          height={1}
          fg={theme.muted}
          selectable={false}
        >
          {gutter}
        </text>
        <text
          width={Math.max(1, width - gutterWidth)}
          height={1}
          content={styled(
            selectedSide === side ? spans.map((s) => ({ ...s, bg })) : spans,
            theme,
            bg,
          )}
          selectable={false}
        />
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
          row.cell.kind === "deletion" ? "left" : "right",
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
