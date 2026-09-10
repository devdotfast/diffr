/** Decode stdout incrementally; reject truncated, reordered, or inconsistent streams. */
import { eventSchema, type DiffEvent } from "./wire";
export async function* readDiffStream(
  chunks: AsyncIterable<Uint8Array>,
): AsyncGenerator<DiffEvent> {
  const decoder = new TextDecoder("utf-8", { fatal: true });
  let buffer = "",
    started = false,
    complete = false,
    succeeded = 0,
    failed = 0,
    total = 0;
  function parse(line: string): DiffEvent {
    if (line.length > 128 * 1024 * 1024)
      throw new Error("diffr event exceeds 128 MiB");
    const event = eventSchema.parse(JSON.parse(line));
    if (complete) throw new Error("Data after diffr completion");
    if (event.type === "start") {
      if (started) throw new Error("Duplicate diffr start");
      started = true;
      total = event.total;
    } else {
      if (!started) throw new Error("Missing diffr start");
      if (event.type === "file") succeeded++;
      if (event.type === "file_error") failed++;
      if (event.type === "complete") {
        if (
          event.succeeded !== succeeded ||
          event.failed !== failed ||
          succeeded + failed !== total
        )
          throw new Error("Inconsistent diffr completion counts");
        complete = true;
      }
    }
    return event;
  }
  for await (const chunk of chunks) {
    buffer += decoder.decode(chunk, { stream: true });
    let end: number;
    while ((end = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, end);
      buffer = buffer.slice(end + 1);
      if (line.trim()) yield parse(line);
    }
    if (buffer.length > 128 * 1024 * 1024)
      throw new Error("diffr event exceeds 128 MiB");
  }
  buffer += decoder.decode();
  if (buffer.trim()) yield parse(buffer);
  if (!complete) throw new Error("Incomplete diffr stream");
}
