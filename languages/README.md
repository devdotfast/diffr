# Extra native languages

`diffr languages list --json` inspects the optional language
pack without networking or loading native code. Install the revision pinned by
your executable with `diffr languages install extra`. For offline installation,
pass `--archive /path/to/package.tgz`; the same catalog checks apply.

**Initial publication is pending.** The checked-in catalog intentionally has no
artifacts until all four signed/final packages have been published and verified.
Until that catalog is incorporated, default builds report `parser_unavailable`.
The per-language `lang-*` features and `all-languages` continue to provide
built-in parsers and take priority over packages. The pack contains Fortran, F#,
Verilog, OCaml (implementation and interface), Julia, Haskell, VHDL, Salesforce
Apex, and QML. LaTeX and general SQL remain bundled.

The single registry is [extra.json](extra.json); see [ADDING.md](ADDING.md) for
the pathway to move another parser out of the built-in configuration.

The store is `<data_local_dir>/diffr/parsers/<executable-target>/extra/<version>`.
`DIFFR_PARSER_DIR` replaces the `parsers` root. Revisions coexist, and ordinary
diffs never download or install anything. A missing, incompatible, or corrupt
parser produces a text diff with a `parser_not_installed`, `parser_unavailable`,
or `parser_load_failed` problem. The wire shape and diff exit codes are unchanged.

## Build and verify

Run on each native target runner (macOS ARM64/x64 or Linux glibc ARM64/x64):

```sh
cargo xtask build-languages --target aarch64-apple-darwin
python3 languages/tests/probe.py target/languages/aarch64-apple-darwin/package
cargo xtask package-languages --target aarch64-apple-darwin
python3 languages/tests/integration.py target/languages/aarch64-apple-darwin/catalog-entry.json --release
```

Python 3 and a C compiler are build tools only. Cargo's locked metadata locates
registry sources; definitions additionally pin versions and source revisions.
Highlight queries travel with the optional pack and are hash-verified before use.
Built-in feature builds continue to use the upstream crate constants. F# uses
only `fsharp/src`, including its scanner, and Fortran includes its scanner.
Licenses come from the pinned crate/source revisions. The builder reads upstream
queries directly from Cargo sources and preserves diffr's existing Julia and
Verilog custom queries. No grammar package contains JavaScript or install scripts.

To exercise x64 execution under Rosetta on an ARM64 Mac, first install the Rust
target with `rustup target add x86_64-apple-darwin`, then run the commands above
with `x86_64-apple-darwin`. The builder and probe explicitly select the target's
C architecture, and the integration test builds and runs the x64 CLI. This also
checks that package selection follows the executable architecture.

On macOS, set `DIFFR_SIGN_IDENTITY` to Whiteboard's Developer ID identity before
running the probe. It signs the libraries and a hardened native host, retaining
library validation. The integration script signs the test CLIs with that identity
as well. Package **after** signing. Packaging never modifies the final libraries.
The integration script temporarily replaces the source catalog to build test
CLIs, then restores it; run it in a dedicated checkout, without concurrent Cargo
builds. There is deliberately no runtime catalog override.

## Publication

Before using the manual `Extra languages` workflow with `publish: true`:

1. Configure the `languages-release` GitHub environment with required reviewers.
   The workflow checks that protection exists and permits publication only from
   `main`.
2. Supply environment secrets `APPLE_CERTIFICATE_BASE64`,
   `APPLE_CERTIFICATE_PASSWORD`, and `NPM_TOKEN`. The certificate must belong to
   Whiteboard's Team ID `PWXY59YDAY`; npm access must cover all four package names.
3. Set the independent revision in `package.template.json`. Do not reuse a
   revision whose bytes have already been published.
4. Run the workflow. All four runners must build, strip, sign where applicable,
   parse with native libraries, and pass installed/static parity before publishing.
5. Download the `verified-language-catalog` artifact, review it, and replace
   `languages/catalog.json` in a normal code change. Release the CLI afterward.

The publisher uploads the exact prepared tarballs and downloads each back for
size/hash verification. An existing immutable revision is accepted only if its
bytes match. It produces no catalog if any target fails. Partial publication can
be retried only with byte-identical artifacts, or with a new pack revision.
Re-signing can change bytes even when source inputs are identical.

Local signed macOS ARM64/Rosetta x64 and Docker Linux ARM64/x64 checks are
recorded in [VALIDATION.md](VALIDATION.md). Gatekeeper behavior for separately
quarantined libraries and public npm installation remain release evidence to
collect; no validation exemption is added to Whiteboard.

## Clean Linux installation test

From the repository root:

```sh
docker build --platform linux/arm64 -f languages/tests/Dockerfile -t diffr-extra-install-test .
docker run --rm --network none --read-only --tmpfs /tmp:rw,exec,nosuid,size=256m diffr-extra-install-test
```

The build stage compiles native Linux artifacts and compares installed parsers
with `all-languages`. The separate Debian runtime contains only diffr, the pack,
fixtures, and base OS utilities. As an unprivileged user, with networking disabled,
it verifies a fresh offline install, every optional grammar, idempotence, corrupt-pack
fallback, and atomic repair. It asserts that Rust, C compilers, Node, npm, and
Python are absent. Switch to `--platform linux/amd64` to exercise the other Linux
target (emulation on ARM hosts is substantially slower).
