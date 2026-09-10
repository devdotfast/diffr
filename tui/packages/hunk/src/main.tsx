/** Run the terminal frontend against a diffr subprocess or saved NDJSON recording. */
import { createReadStream, openSync, closeSync } from "node:fs";
import { ReadStream } from "node:tty";
import { spawn } from "node:child_process";
import { createCliRenderer } from "@opentui/core";
import { createRoot } from "@opentui/react";
import { readDiffStream } from "./diffr/stream";
import { DiffStore } from "./diffr/store";
import { App } from "./ui/App";
const args = process.argv.slice(2),
  store = new DiffStore();
let child: ReturnType<typeof spawn> | undefined;
let chunks: AsyncIterable<Uint8Array>;
let input: NodeJS.ReadStream = process.stdin;
let ttyFd: number | undefined;
let expectsDifferenceExit = false;
let comparisonExitCode = 0;
if (args[0] === "--input" && args[1]) {
  chunks = args[1] === "-" ? process.stdin : createReadStream(args[1]);
  if (args[1] === "-") {
    ttyFd = openSync(process.platform === "win32" ? "CONIN$" : "/dev/tty", "r");
    input = new ReadStream(ttyFd);
  }
} else if (args[0] === "--diffr" && args[1]) {
  const comparison = args.slice(args[2] === "--" ? 3 : 2);
  const separator = comparison.indexOf("--");
  expectsDifferenceExit = comparison
    .slice(0, separator < 0 ? comparison.length : separator)
    .includes("--exit-code");
  child = spawn(args[1], ["--format", "ndjson", ...comparison], {
    stdio: ["ignore", "pipe", "pipe"],
  });
  chunks = child.stdout!;
} else {
  console.error(
    "Usage: bun run start --diffr /path/to/diffr -- [comparison arguments]\n       bun run start --input recording.ndjson (or -)",
  );
  process.exit(2);
}
const renderer = await createCliRenderer({
  stdin: input,
  useMouse: true,
  enableMouseMovement: true,
  exitOnCtrlC: false,
  screenMode: "alternate-screen",
});
let quitting = false;
function quit() {
  if (quitting) return;
  quitting = true;
  child?.kill();
  renderer.destroy();
  if (ttyFd !== undefined) {
    input.destroy();
    try {
      closeSync(ttyFd);
    } catch {}
  }
  process.exit(store.getSnapshot().errors.length ? 2 : comparisonExitCode);
}
process.once("SIGTERM", quit);
process.once("SIGINT", quit);
const root = createRoot(renderer);
root.render(<App store={store} onQuit={quit} />);
let stderr = "";
child?.stderr?.on("data", (data) => {
  stderr = (stderr + data.toString()).slice(-16384);
});
child?.on("error", (error) => store.fail(error));
child?.on("close", (code) => {
  if (code === 1 && expectsDifferenceExit) {
    comparisonExitCode = 1;
    return;
  }
  if (!quitting && code !== 0 && code !== null)
    store.fail(stderr || `diffr exited with status ${code}`);
});
try {
  for await (const event of readDiffStream(chunks!)) {
    if (quitting) break;
    store.accept(event);
  }
} catch (error) {
  if (!quitting) {
    store.fail(stderr || error);
    child?.kill();
  }
}
