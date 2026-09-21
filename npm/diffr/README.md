# @dev.fast/diffr

diffr's NDJSON wire contract as TypeScript with zod validators, plus a
fetcher for the matching release binary. The package version is the diffr
release it describes.

```ts
import { decodeStructuralDiffEvent } from "@dev.fast/diffr";
for await (const line of lines) {
  const event = decodeStructuralDiffEvent(line); // throws on any shape drift
}
```

```sh
npx --package @dev.fast/diffr@0.1.1 diffr-fetch --into ./bin
npx --package @dev.fast/diffr@0.1.1 diffr-fetch --into ./bin --check
```

Supports macOS arm64 and Linux x64. `--check` never downloads; `--required`
makes network failures fatal. Without it, network failures warn and exit 0.
Hash mismatches and invalid archives always fail.

## Files that change together

A wire-format change is not done until every row is updated.

| Source of truth | Mirror | Checked by |
|---|---|---|
| `src/protocol/mod.rs`, `src/pairing.rs` | `npm/diffr/src/contract.ts` | `contract.test.ts` runs the built `diffr` and validates every record |
| `src/protocol/mod.rs` `VERSION` | `contract.ts` `STRUCTURAL_DIFF_BASE_WIRE_VERSION` | `version.test.ts` |
| `docs/streaming.md` | `contract.ts` doc comments | review |
| `src/protocol/mod.rs` | `tui/packages/hunk/src/diffr/wire.ts` (v3 only) | `tui` tests on `tui/test/fixtures/comparison.ndjson` |
| this package's version | consumers' exact-version dependency (Review: `packages/review-protocol` and `packages/review`) | the consumer's bump PR |

`contract.ts` imports only zod. Consumers concatenate it into generated
source where zod is the only external, so keep it that way.

## Releasing

1. Merge the Rust and `contract.ts` change together.
2. Tag the release; wait for the archives.
3. From `npm/diffr`, `npm run pin` writes `pins.json` for this version.
4. Build the Rust binary, then run `bun install --frozen-lockfile`,
   `bun run typecheck`, and `bun test` in the package directory.
5. `npm publish --access public` builds the package before packing.
