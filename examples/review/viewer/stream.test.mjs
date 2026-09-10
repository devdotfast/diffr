import { test } from "node:test";
import assert from "node:assert/strict";
import { diffEvents } from "./stream.mjs";

function response(text) {
  const bytes = new TextEncoder().encode(text);
  return new Response(new ReadableStream({
    start(controller) {
      // Split inside both UTF-8 characters and JSON records.
      for (const byte of bytes) controller.enqueue(new Uint8Array([byte]));
      controller.close();
    },
  }));
}
test("decodes split records and UTF-8, verifies completion", async () => {
  const events = [
    { type: "start", version: 1, total: 1 },
    { type: "file", diff: "☕" },
    { type: "complete", succeeded: 1, failed: 0 },
  ];
  globalThis.fetch = async () => response(events.map(JSON.stringify).join("\n") + "\n");
  assert.deepEqual(await Array.fromAsync(diffEvents({})), events);
});
test("rejects truncation and inconsistent counts", async () => {
  for (const tail of ["", '{"type":"complete","succeeded":0,"failed":0}\n']) {
    globalThis.fetch = async () => response('{"type":"start","version":1,"total":1}\n' + tail);
    await assert.rejects(async () => { for await (const event of diffEvents({})) void event; });
  }
});
