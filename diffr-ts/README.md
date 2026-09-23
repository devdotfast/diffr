# @dev.fast/diffr

TypeScript types, zod validators, and a binary fetcher for diffr's NDJSON protocol.
The package version matches the diffr release.

```ts
import { decodeStructuralDiffEvent } from "@dev.fast/diffr";
const event = decodeStructuralDiffEvent(line);
```

```sh
npx --package @dev.fast/diffr@0.1.3 diffr-fetch --into ./bin --required
```

Supports macOS arm64 and Linux x64. `--check` verifies an existing install without
network access. Downloads warn on network failure unless `--required`; invalid
hashes or archives always fail.

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
run from `diffr-ts`:

```sh
bun install --frozen-lockfile
npm run pin
bun run typecheck && bun test
npm publish --access public
```
