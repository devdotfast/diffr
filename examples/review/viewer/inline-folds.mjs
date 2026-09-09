// Project byte ranges onto a source line. Offsets remain UTF-8 byte offsets.
export function foldLine(length, row, folds, closed) {
  const spans = folds
    .filter(
      ({ range: r }) =>
        r.start.line <= row &&
        (row < r.end.line || (row === r.end.line && r.end.byte_column > 0)),
    )
    .map(({ id, range: r }) => ({
      id,
      start: r.start.line === row ? r.start.byte_column : 0,
      end: r.end.line === row ? r.end.byte_column : length,
      startsOnRow: r.start.line === row,
    }))
    .sort((a, b) => a.start - b.start || b.end - a.end || a.id - b.id);
  const result = [];
  // Cursor tracks emitted source; hiddenUntil also suppresses nested controls.
  let cursor = 0,
    hiddenUntil = -1;
  for (const span of spans) {
    if (span.start < hiddenUntil) continue;
    if (span.start > cursor) result.push({ text: [cursor, span.start] });
    cursor = Math.max(cursor, span.start);
    if (span.startsOnRow)
      result.push({ fold: span.id, collapsed: closed.has(span.id) });
    if (closed.has(span.id)) {
      cursor = Math.max(cursor, span.end);
      hiddenUntil = span.end;
    }
  }
  if (cursor < length) result.push({ text: [cursor, length] });
  return result;
}
