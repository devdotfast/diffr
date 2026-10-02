/**
 * Highlight spans packed into typed arrays. A large change carries millions of spans; as objects they
 * cost more memory than the source itself, and copying them out of the worker would stall the page.
 * Packed, they are three numbers each in a buffer the worker hands over without a copy.
 */
import type { SyntaxSpan } from "../../tui/packages/hunk/src/diffr/wire";

export interface Syntax {
  /** Capture names, each once; spans refer to them by index. */
  captures: string[];
  /** Three numbers a span, in line order: start byte, end byte, capture index. */
  spans: Uint32Array;
  /** The first span of each line, and one past the last line's. */
  index: Uint32Array;
}

export function packSyntax(spans: SyntaxSpan[], lines: number): Syntax {
  const captures: string[] = [];
  const ids = new Map<string, number>();
  const sorted = spans.length > 1 && spans.some((s, i) => i > 0 && s.line < spans[i - 1]!.line)
    ? [...spans].sort((a, b) => a.line - b.line) : spans;
  const packed = new Uint32Array(sorted.length * 3);
  const index = new Uint32Array(lines + 1);
  let line = 0;
  sorted.forEach((span, i) => {
    let id = ids.get(span.capture);
    if (id === undefined) {
      id = captures.length;
      captures.push(span.capture);
      ids.set(span.capture, id);
    }
    packed[i * 3] = span.start_column;
    packed[i * 3 + 1] = span.end_column;
    packed[i * 3 + 2] = id;
    while (line < span.line && line < lines) index[++line] = i;
  });
  while (line < lines) index[++line] = sorted.length;
  return { captures, spans: packed, index };
}

/** The spans of one line, as [start, end, capture] triples. */
export function lineSpans(syntax: Syntax | undefined, line: number): [number, number, string][] {
  if (!syntax || line + 1 >= syntax.index.length) return [];
  const out: [number, number, string][] = [];
  for (let i = syntax.index[line]!, end = syntax.index[line + 1]!; i < end; i++)
    out.push([syntax.spans[i * 3]!, syntax.spans[i * 3 + 1]!, syntax.captures[syntax.spans[i * 3 + 2]!]!]);
  return out;
}
