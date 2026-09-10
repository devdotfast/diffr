// A response ending without "complete" is incomplete, even with HTTP 200.
export async function* diffEvents(request, signal) {
  const response = await fetch("/diff", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(request),
    signal,
  });
  if (!response.ok) throw Error(await response.text());
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let pending = "", started = false, complete = false, total = 0, succeeded = 0, failed = 0;
  try {
    while (true) {
      const { value, done } = await reader.read();
      pending += decoder.decode(value, { stream: !done });
      let end;
      while ((end = pending.indexOf("\n")) >= 0) {
        const line = pending.slice(0, end);
        pending = pending.slice(end + 1);
        if (!line.trim()) continue;
        const event = JSON.parse(line);
        if (complete) throw Error("Data after stream completion");
        if (!started && (event.type !== "start" || event.version !== 1)) throw Error("Unsupported stream");
        if (event.type === "start") {
          if (started) throw Error("Repeated start event");
          total = event.total;
        } else if (event.type === "file") {
          succeeded++;
        } else if (event.type === "file_error") {
          failed++;
        } else if (event.type === "error") {
          throw Error(event.message);
        } else if (event.type === "complete") {
          if (event.succeeded !== succeeded || event.failed !== failed || succeeded + failed !== total)
            throw Error("Inconsistent completion counts");
          complete = true;
        } else {
          throw Error("Unknown stream event");
        }
        started = true;
        yield event;
      }
      if (done) break;
    }
    if (pending.trim() || !complete) throw Error("Diff stream ended before completion");
  } finally {
    await reader.cancel();
    reader.releaseLock();
  }
}
