# @dev.fast/diffr

TypeScript types, zod validators, and a binary fetcher for diffr's NDJSON protocol.
The package version matches the diffr release.

```ts
import { decodeStructuralDiffEvent } from "@dev.fast/diffr";
const event = decodeStructuralDiffEvent(line);
```

```sh
npx --package @dev.fast/diffr@0.1.7 diffr-fetch --into ./bin --required
```

Supports macOS arm64, macOS x64, Linux arm64/glibc, and Linux x64/glibc. `--check` verifies an existing install without
network access. Downloads warn on network failure unless `--required`; invalid
hashes or archives always fail.

## Choosing languages at runtime

The default `lean` edition excludes Apex, Fortran, F#, Haskell, Julia, OCaml
(including interfaces), QML, Verilog, and VHDL. It uses text diffs for those files.
The `full` edition compiles all supported languages. Both use the same wire format.

```ts
import { ensureBinary } from "@dev.fast/diffr/binary";
import { spawn } from "node:child_process";

// Call when the user selects an edition; retain the returned path for comparisons.
const executable = await ensureBinary({
  directory: appDataDirectory,
  edition: "full", // or "lean"
});
spawn(executable, ["--repo", repository, "--format", "ndjson", "HEAD"]);
```

The API downloads the package's exact pinned release and process architecture,
checks the archive hash, and stores editions separately under
`<directory>/<version>/<target>/<edition>/diffr`. Installed binaries are checked
for corruption and reused offline. `check: true` prohibits downloads. Select a new
path only after the promise succeeds; existing comparisons finish on their old
executable. Clear application diff caches when switching. Do not replace files
inside a signed application bundle.

The CLI also accepts `--edition full`; `--into` still writes `diffr` directly into
the given directory for existing build scripts. Use different directories to keep
both editions. Neither API needs Rust, npm, or a compiler at runtime; extraction
uses the operating system's `tar`.

Full downloads require a release with full-edition pins. The existing 0.1.7 pins
remain unchanged until matching artifacts are published; requesting an unpinned
edition fails rather than downloading a different version.

## Wire changes

Keep these files in sync:

| Files (from repo root) | Check |
|---|---|
| `src/protocol/mod.rs`, `src/pairing.rs`, `diffr-ts/src/contract.ts` | Contract tests against the binary; Rust version check |
| `docs/streaming.md` | Review against the wire format |
| `tui/packages/hunk/src/diffr/wire.ts` (v3 only) | TUI fixture tests |
| Review's `packages/review-protocol/package.json` and `packages/review/package.json` | Exact package version pins |

## Release

After tagging the matching Rust release and building with `cargo build --locked`,
run from `diffr-ts`. Pinning downloads all eight CLI archives (two editions on
four targets) and writes the catalog only after every download succeeds. Legacy
single-edition pins remain supported:

```sh
bun install --frozen-lockfile
npm run pin
bun run typecheck && bun test
npm publish --access public
```
