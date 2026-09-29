#!/usr/bin/env node
import { parseArgs } from "node:util";
import { installBinary } from "./binary.mjs";

try {
  const { values } = parseArgs({ options: {
    into: { type: "string" },
    edition: { type: "string", default: "lean" },
    check: { type: "boolean" },
    required: { type: "boolean" },
    pins: { type: "string" },
  } });
  if (!values.into) throw new Error("--into <dir> is required");
  await installBinary({ into: values.into, edition: values.edition, check: values.check,
    required: values.required, pinsPath: values.pins });
} catch (error) {
  console.error(`diffr-fetch: ${error.message}`);
  process.exitCode = 1;
}
