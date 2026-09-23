# Releases and Homebrew

The release workflow builds `diffr` and the standalone `diffr-tui` on Apple Silicon
macOS and x86-64 Linux. Bun 1.3.10 is required only when building. Each archive
contains both executables at its root, plus license notices. Existing consumers
can continue extracting only `diffr` using the original asset naming convention.

Before tagging, update the root package version in Cargo.toml and its Cargo.lock
entry. Keep plugin dependency versions unchanged unless those crates also change.
The tag must match the CLI version exactly (`X.Y.Z`). Do not replace old assets:
consumers pin their checksums.

Release packaging runs on relevant pull requests and can be run manually without
publishing. Each platform checks an extracted archive in a fresh directory with an
empty PATH, exercising settings and interactive diff rendering without Bun or the
checkout. The workflow generates `SHA256SUMS` and `diffr.rb` from both archives.
Only after validation does a tag build upload a draft release and publish it.

If upload fails, inspect and remove the incomplete draft before rerunning the
publish job. Never delete or overwrite an already public release to retry it.

The public `devdotfast/homebrew-tap` repository checks the latest release hourly
and installs/tests its formula before committing an update. Its workflow also has
a manual trigger. This uses the tap's own GitHub token, with no cross-repository
credential. Releases predating the `diffr.rb` asset are ignored.

Install with `brew install devdotfast/tap/diffr`; update with `brew update` followed
by `brew upgrade devdotfast/tap/diffr`. To install manually, verify the archive with
`SHA256SUMS`, extract it, and copy both executables into the same directory on PATH.
