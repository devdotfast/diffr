/**
 * The main thread's handle on diffr: the wasm module is fetched and compiled once here, then handed
 * to a few workers, so files diff in parallel without each worker paying for its own compile.
 */
// Gzipped by the build (vite.config.ts): static hosts cap a file at 25 MiB, and the engine is 64.
import wasmUrl from "./wasm/diffr_web_bg.wasm.gz?url";
import type { Status } from "./github";
import type { Diffed, Request, Response } from "./worker";

export interface Side {
  path: string;
  text: string;
}

/** What the engine panel shows. */
export const engineStats = {
  workers: Math.max(1, Math.min(8, (navigator.hardwareConcurrency || 4) - 2)),
  /** Milliseconds from page start until the module was compiled. */
  compiled: 0,
  /** Milliseconds from page start until the first worker could diff. */
  ready: 0,
  bytes: 0,
};

export type Result = Diffed & { ms: number };
type Call = { resolve: (result: Result) => void; reject: (error: Error) => void };

interface Slot {
  worker: Worker;
  busy: number;
  /** Its wasm memory grew past the limit: replace it once its queue drains. */
  retire: boolean;
}

/** Wasm memory never shrinks, so a worker that has diffed one huge file is replaced afterwards. */
const MEMORY_LIMIT = 512 * 1024 * 1024;

const pending = new Map<number, Call>();
const owner = new Map<number, Slot>();
let next = 0;
const listeners: (() => void)[] = [];

export function onEngineChange(listener: () => void) {
  listeners.push(listener);
}

/**
 * The engine's bytes, unpacked as they arrive so compiling still streams. A server that sent the
 * file with `Content-Encoding: gzip` has unpacked it already, so the first bytes decide.
 */
async function unpacked(response: globalThis.Response): Promise<globalThis.Response> {
  const [peek, body] = response.body!.tee();
  const reader = peek.getReader();
  const { value } = await reader.read();
  void reader.cancel();
  const gzipped = !!value && value[0] === 0x1f && value[1] === 0x8b;
  const stream = gzipped ? body.pipeThrough(new DecompressionStream("gzip")) : body;
  return new globalThis.Response(stream, { headers: { "content-type": "application/wasm" } });
}

const module = (async () => {
  const response = await fetch(wasmUrl);
  if (!response.ok) throw new Error(`the diffr engine failed to load (${response.status})`);
  engineStats.bytes = Number(response.headers.get("content-length")) || 0;
  const compiled = await WebAssembly.compileStreaming(await unpacked(response));
  engineStats.compiled = performance.now();
  return compiled;
})();

/** Two to start; more join while files queue up, so a small change never pays for eight engines. */
const slots: Slot[] = Array.from({ length: Math.min(2, engineStats.workers) }, () => spawn());

function spawn(): Slot {
  const slot: Slot = { worker: new Worker(new URL("./worker.ts", import.meta.url), { type: "module" }), busy: 0, retire: false };
  void module.then((compiled) => slot.worker.postMessage({ module: compiled } satisfies Request));
  slot.worker.onmessage = ({ data }: MessageEvent<Response>) => {
    if ("ready" in data) {
      if (!engineStats.ready) engineStats.ready = performance.now();
      listeners.forEach((listener) => listener());
      return;
    }
    slot.busy--;
    if (data.memory > MEMORY_LIMIT) slot.retire = true;
    if (slot.retire && !slot.busy) {
      slot.worker.terminate();
      slots[slots.indexOf(slot)] = spawn();
    }
    const call = pending.get(data.id)!;
    pending.delete(data.id);
    owner.delete(data.id);
    if ("error" in data) return call.reject(new Error(data.error));
    call.resolve(data);
  };
  slot.worker.onerror = (event) => {
    for (const [id, call] of pending)
      if (owner.get(id) === slot) {
        call.reject(new Error(event.message || "the diffr worker failed to start"));
        pending.delete(id);
      }
  };
  return slot;
}

module.catch((error: unknown) => {
  for (const call of pending.values()) call.reject(error instanceof Error ? error : new Error(String(error)));
  pending.clear();
});

/**
 * The change is fully diffed: let every worker go but one, fresh, since each holds wasm memory as
 * large as the largest file it saw.
 */
export function release() {
  for (const slot of slots.splice(0)) {
    if (slot.busy) slots.push(slot);
    else slot.worker.terminate();
  }
  if (!slots.length) slots.push(spawn());
}

/** Workers running now. */
export const activeWorkers = () => slots.length;

/** Diff one file on the least busy worker. Resolves with diffr's record and the time it took. */
export function diff(status: Status, lhs: Side | undefined, rhs: Side | undefined): Promise<Result> {
  const id = next++;
  // A retiring worker takes nothing new, so it drains and goes.
  let slot = slots.reduce((a, b) => (Number(b.retire) * 1e9 + b.busy < Number(a.retire) * 1e9 + a.busy ? b : a));
  if (slot.busy >= 2 && slots.length < engineStats.workers) slots.push((slot = spawn()));
  slot.busy++;
  owner.set(id, slot);
  const request: Request = { id, request: JSON.stringify({ status, lhs, rhs, syntax: true }) };
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    // After the module: each worker's first message must be the module it runs.
    void module.then(() => slot.worker.postMessage(request));
  });
}
