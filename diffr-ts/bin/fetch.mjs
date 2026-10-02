#!/usr/bin/env node
import { parseArgs } from "node:util";
import { ensureBinary } from "./binary.mjs";
try {
  const { values } = parseArgs({ options: {
    into: { type: "string" }, full: { type: "boolean", default: false },
    check: { type: "boolean" }, required: { type: "boolean", default: false }, pins: { type: "string" },
  } });
  if (!values.into) throw new Error("--into <dir> is required");
  const binary = await ensureBinary(Object.fromEntries(Object.entries(values).filter(([, value]) => value !== undefined)));
  if (binary) console.log(`diffr installed at ${binary}`);
} catch (error) { console.error(`diffr-fetch: ${error.message}`); process.exitCode = 1; }
