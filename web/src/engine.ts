/** The main thread's handle on the diffr worker. */
import { eventSchema, type FileEvent } from "../../tui/packages/hunk/src/diffr/wire";
import type { Status } from "./github";
import type { Request, Response } from "./worker";

export interface Side {
  path: string;
  text: string;
}

const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
const pending = new Map<number, { resolve: (event: FileEvent) => void; reject: (error: Error) => void }>();
let next = 0;

worker.onmessage = ({ data }: MessageEvent<Response>) => {
  const call = pending.get(data.id)!;
  pending.delete(data.id);
  if ("error" in data) return call.reject(new Error(data.error));
  // diffr omits defaults on the wire; the schema puts them back.
  const event = eventSchema.parse(JSON.parse(data.ok).event);
  if (event.type !== "file") return call.reject(new Error(`diffr sent a ${event.type} event for a file`));
  call.resolve(event);
};

worker.onerror = (event) => {
  for (const call of pending.values()) call.reject(new Error(event.message || "the diffr worker failed to start"));
  pending.clear();
};

export function diff(status: Status, lhs: Side | undefined, rhs: Side | undefined): Promise<FileEvent> {
  const id = next++;
  const request: Request = { id, request: JSON.stringify({ status, lhs, rhs, syntax: true }) };
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject });
    worker.postMessage(request);
  });
}
