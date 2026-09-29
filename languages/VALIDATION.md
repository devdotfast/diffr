# Local validation — 2026-09-29

Base: diffr `58fa3b2` (includes PR #49). Host: macOS ARM64. No packages were
published, and `catalog.json` remains empty pending verified publication.

## Registry/query refactor — pack 0.3.0

`extra.json` now generates the Rust configuration and drives packaging. Upstream
highlight files are no longer copied into the repository or embedded in default
builds. The pack contains catalog-hashed queries alongside its libraries;
`all-languages` still reads upstream crate constants.

- Full default workspace tests: 290 passed, one ignored.
- Default and all-languages bin/CLI tests: 271 passed, one ignored, in each build.
- Five registry checks and both publication-gate tests passed.
- Signed macOS ARM64 native/static parity passed for all ten grammars, including
  query corruption/fallback/repair, scanners, parallel installs/diffs, offline
  installation, the default store, quarantine-marked archives, and old CLI reuse.
- Linux ARM64 Docker passed the same parity suite and the minimal unprivileged,
  read-only-root, no-network/no-development-tools runtime installation test.
- The source package includes `extra.json` and the catalog, with no copied upstream
  highlight files. Formatting, actionlint, typos, and whitespace checks passed.
- Clippy reports only the six pre-existing warnings listed below.

Compressed pack sizes: 3,947,947 bytes on macOS ARM64 and 3,748,052 bytes on Linux
ARM64. The four-target workflow will rerun on the updated PR; the x64 results
below describe the previous 0.2.0 pack, before query relocation.

## Earlier 0.2.0 validation

## Passed

- `cargo test --locked`: 290 tests passed, one ignored.
- `cargo test --locked --features all-languages --bin diffr --test cli`:
  271 passed, one ignored.
- Archive/download tests cover interrupted and oversized transfers, wrong
  hashes/targets/sizes, missing files, duplicates, symlinks, hardlinks, nested
  paths, and traversal.
- `python3 languages/tests/test_publish.py`: both publication-gate tests passed.
- `actionlint .github/workflows/languages.yml`, formatting, and `git diff --check`.
- Cargo's package file list includes the embedded catalog, all ten highlight
  queries, and their licenses.
- All ten C grammars built and parsed in a signed hardened native probe.
- Signed release CLIs passed installed/static NDJSON and syntax parity, tree
  comparison, scanner-heavy Fortran/F# fixtures, added/deleted/unchanged files,
  120 mixed-language files with eight workers, concurrent installers, corruption,
  repair, offline reuse, and revision coexistence.
- A fresh offline install and diff succeeded with an empty `PATH`.
- The signed macOS CLI passed the default `Library/Application Support` store,
  an archive marked with `com.apple.quarantine`, truncated-archive rejection,
  and actual coexistence with the previously built 0.1.0 CLI and pack.
- Definition checks (three tests), shared OCaml feature coverage, and the
  metadata-driven packaging and fixture discovery checks passed.

Both the probe and full CLI used Team ID `PWXY59YDAY`. The full CLI used the
existing Whiteboard diffr entitlement `allow-unsigned-executable-memory`.
Neither host disabled library validation. Loading was tested from the parser
store outside the application bundle. The original Whiteboard app was not changed.

Clippy completes with six pre-existing warnings in `src/diff/graph.rs`,
`src/parse/syntax.rs`, and `src/protocol/mod.rs`; `-D warnings` fails on those.

## Measurements

One local signed ARM64 pack, revision `0.2.0`, containing ten grammars:

| Measurement | Bytes |
|---|---:|
| Compressed pack | 3,932,587 |
| Native-pack release CLI | 93,029,568 |
| All-languages release CLI | 148,755,536 |

The executable is 55,725,968 bytes smaller than the equivalent all-languages
build. Both measurements include Developer ID signatures. These are local release
executables, not publicly downloaded CLI artifacts.

Installer/loading overhead was separately measured before the pack expansion,
with the original three-parser feature set held constant: 107,839,232 bytes for
the baseline and 112,555,376 for the installer/loader build, an increase of
4,716,144 bytes (4.37%). That comparison isolates installer work; it is not the
expanded pack's net size change.

Ten new-process Apex diffs had a median of 196 ms; the first measured sample
was 345 ms. A 120-file mixed batch with eight workers took 777 ms. Each process
starts with an empty parser cache, while the batch reuses loaded grammars.
The OS page cache was warm; these are not cold-disk measurements. Timing is
indicative, not a performance gate.

## Outstanding release evidence

Public npm downloads and behavior for separately quarantined libraries have
not been exercised locally. The
four-runner workflow must pass before publication. The repository currently
has no configured signing/npm secrets or protected publication environment;
see README.md for the exact setup. Only a catalog emitted after all four
registry downloads match may replace the empty checked-in catalog.

## Linux ARM64 Docker verification

A native `aarch64-unknown-linux-gnu` build passed `languages/tests/probe.py` and
the complete installed/static integration suite in Docker. The Linux pack was
3,737,483 bytes compressed (revision `0.2.0`). The separate Debian Bookworm runtime passed
`container-smoke.sh` with networking disabled, a read-only root filesystem,
a temporary writable parser store, and UID/GID 65534.

The Linux native CLI was 98,161,256 bytes; the all-languages CLI was
153,820,688 bytes. Ten new-process Apex diffs had a median of 228 ms,
and the 120-file mixed batch took 1,554 ms.

The runtime verifies absence of Cargo, Rust, C compilers, Node, npm, and Python;
then tests fresh offline installation, all ten structural parsers, idempotence,
corrupt-library text fallback, and atomic repair. This test is also wired into
both Linux jobs in `languages.yml`. The local run covers ARM64 only.

Commands:

```sh
docker build --platform linux/arm64 -f languages/tests/Dockerfile -t diffr-extra-install-test .
docker run --rm --network none --read-only --tmpfs /tmp:rw,exec,nosuid,size=256m diffr-extra-install-test
```

Result: `PASS: fresh offline install, all optional parsers, idempotence, corrupt
fallback, atomic repair; unprivileged and without development tools.`

The first attempted build used a manually assembled context that omitted tracked
grammar-directory symlinks. Rebuilding with the ordinary Docker context and the
provided Dockerfile-specific ignore file fixed the test setup. No parser or
installer fix was needed. Public npm installation remains unverified until
publication and catalog activation.

## macOS x64 / Rosetta verification

The signed `x86_64-apple-darwin` probe and both signed x64 CLIs ran under Rosetta
on the ARM64 host. All ten grammars passed native/static diff, syntax, and tree
parity, scanner-heavy fixtures, added/deleted/unchanged files, the 120-file batch,
concurrent installation, corruption/repair, truncated archive rejection, the
default Application Support store, quarantined-archive installation, and an
empty-PATH fresh install. The executable selected the x64 package.

Both x64 hosts and libraries use Team ID `PWXY59YDAY` with hardened runtime;
the full CLI retains only Whiteboard's existing executable-memory entitlement.
An additional shared-store check installed both ARM64 and x64 revisions into
one root and confirmed that each executable selected its own target and parsed
QML without fallback.

| x64 measurement | Bytes |
|---|---:|
| Compressed pack | 4,219,033 |
| Native-pack release CLI | 96,633,808 |
| All-languages release CLI | 152,322,048 |

The new-process Apex median was 395 ms; the 120-file mixed batch took 1,800 ms.
These are Rosetta timings with warm OS caches, not native Intel benchmarks.

All four versioned npm tarball URLs returned HTTP 404 on 2026-09-29. Public
network installation remains blocked on publication. The repository has no
signing/npm repository secrets and no protected `languages-release` environment;
only the unprotected test environment `languages-ci` currently exists.

## Linux x64 Docker verification

The `linux/amd64` Docker build passed all ten native probes and the complete
installed/static integration suite under emulation on the ARM64 host. Its separate
minimal runtime passed fresh offline installation, all parsers, idempotence,
corrupt-library fallback, and atomic repair with UID/GID 65534, networking disabled,
a read-only root, and no Rust, C compiler, Node, npm, or Python.

The compressed pack was 3,759,226 bytes; native and all-languages executables
were 100,672,240 and 156,206,248 bytes. The Apex median was 366 ms and the
120-file mixed batch took 2,434 ms; these are warm-cache emulation measurements.

The native Linux x64 GitHub runner also passed installation/parity and its
minimal Docker runtime check in run `36609243307`, alongside both ARM64 runners.
The macOS Intel job was still running when these local results were recorded.
