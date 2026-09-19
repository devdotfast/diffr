import { execFileSync } from "node:child_process";
import { copyFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { join } from "node:path";

const cwd = fileURLToPath(new URL("../../", import.meta.url));
execFileSync("cargo", ["build", "-p", "diffr-node"], { cwd, stdio: "inherit" });
const metadata = JSON.parse(execFileSync("cargo", ["metadata", "--no-deps", "--format-version=1"], { cwd }));
const library = process.platform === "darwin" ? "libdiffr_node.dylib"
  : process.platform === "win32" ? "diffr_node.dll" : "libdiffr_node.so";
copyFileSync(join(metadata.target_directory, "debug", library),
  new URL("./diffr.node", import.meta.url));
