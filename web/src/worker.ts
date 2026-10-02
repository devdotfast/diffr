/// <reference lib="webworker" />
/** Runs diffr off the main thread: one engine, one file at a time, in request order. */
import { eventSchema, type FileEvent, type Region } from "../../tui/packages/hunk/src/diffr/wire";
import { lineCount, splitLines, type Lines } from "./lines";
import { packSyntax, type Syntax } from "./syntax";
import init, { Differ } from "./wasm/diffr_web.js";

/** A diffed file as the page keeps it: the text in lines and the highlights packed, out of the event. */
export interface Diffed {
  event: FileEvent;
  lines: [Lines, Lines];
  syntax: [Syntax | undefined, Syntax | undefined];
}

/** The first message carries the compiled module; every later one is a file to diff. */
export type Request = { module: WebAssembly.Module } | { id: number; request: string };
export type Response =
  | { ready: number }
  | ({ id: number; ms: number; memory: number } & Diffed)
  | { id: number; error: string; ms: number; memory: number };

/**
 * A leaf with no counterpart on the other side is new or gone from end to end, and is drawn without
 * word marks; diffr still marks every token in it. Only whether it has any marks matters, so one stays.
 */
function trimUnpaired(sides: [Region[], Region[]]) {
  const leaves = sides.map((regions) => {
    const all: Region[] = [];
    const walk = (list: Region[]) => list.forEach((r) => (r.kind === "leaf" ? all.push(r) : walk(r.children)));
    walk(regions);
    return all;
  });
  const alignments = leaves.map((list) => new Set(list.map((r) => (r.kind === "leaf" ? r.alignment_id : -1))));
  leaves.forEach((list, side) => {
    for (const leaf of list)
      if (leaf.kind === "leaf" && leaf.changed.length > 1 && !alignments[side ? 0 : 1]!.has(leaf.alignment_id))
        leaf.changed = leaf.changed.slice(0, 1);
  });
}

/**
 * diffr's default graph limit, 3 million, sends heavy rewrites of ordinary source files to a line
 * diff. Ten million matches them; the engine keeps such files to one worker at a time.
 */
const GRAPH_LIMIT = 10_000_000;

let differ: Promise<Differ> | undefined;
let memory: WebAssembly.Memory | undefined;

self.onmessage = async ({ data }: MessageEvent<Request>) => {
  if ("module" in data) {
    const start = performance.now();
    differ = init({ module_or_path: data.module }).then((wasm) => {
      memory = wasm.memory;
      const engine = new Differ();
      engine.setGraphLimit(GRAPH_LIMIT);
      return engine;
    });
    await differ;
    self.postMessage({ ready: performance.now() - start } satisfies Response);
    return;
  }
  const engine = await differ!;
  const start = performance.now();
  let reply: Response;
  try {
    // diffr omits defaults on the wire; the schema puts them back. Parsing here keeps it off the page's thread.
    const event = eventSchema.parse(JSON.parse(engine.diff(data.request)).event);
    if (event.type !== "file") throw new Error(`diffr sent a ${event.type} event for a file`);
    const lines: Diffed["lines"] = [splitLines(""), splitLines("")];
    const syntax: Diffed["syntax"] = [undefined, undefined];
    const diff = event.diff;
    if (diff?.type === "text") trimUnpaired([diff.lhs?.regions ?? [], diff.rhs?.regions ?? []]);
    if (diff?.type === "text")
      ([diff.lhs, diff.rhs] as const).forEach((side, i) => {
        if (!side) return;
        lines[i] = splitLines(side.text);
        if (side.syntax.length) syntax[i] = packSyntax(side.syntax, lineCount(lines[i]));
        side.text = "";
        side.syntax = [];
      });
    reply = { id: data.id, event, lines, syntax, ms: performance.now() - start, memory: memory?.buffer.byteLength ?? 0 };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    reply = { id: data.id, error: message, ms: performance.now() - start, memory: memory?.buffer.byteLength ?? 0 };
  }
  const transfer = "syntax" in reply
    ? [...reply.syntax.flatMap((s) => (s ? [s.spans.buffer, s.index.buffer] : [])), ...reply.lines.map((l) => l.starts.buffer)]
    : [];
  self.postMessage(reply, transfer);
};
