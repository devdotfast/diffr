/// <reference lib="webworker" />
/** Runs diffr off the main thread: one engine, one file at a time, in request order. */
import init, { Differ } from "./wasm/diffr_web.js";

export type Request = { id: number; request: string };
export type Response = { id: number; ok: string } | { id: number; error: string };

const differ = init().then(() => new Differ());

self.onmessage = async ({ data }: MessageEvent<Request>) => {
  let reply: Response;
  try {
    reply = { id: data.id, ok: (await differ).diff(data.request) };
  } catch (error) {
    reply = { id: data.id, error: error instanceof Error ? error.message : String(error) };
  }
  self.postMessage(reply);
};
