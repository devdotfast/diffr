#!/bin/sh
# Build plugin crates into their folders' plugin.wasm: the bundled plugins
# that also run as components (plugins/<name>, the same source diffr's native
# registry compiles in) and the examples (examples/plugins). Needs the
# wasm32-wasip2 target for the pinned toolchain:
#   rustup target add wasm32-wasip2
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
target="$root/target/wasm-plugins"

# build <package> <plugin folder> <library name>
build() {
    cargo rustc --quiet --release --target wasm32-wasip2 --crate-type cdylib \
        --manifest-path "$root/Cargo.toml" --package "$1" --target-dir "$target"
    cp "$target/wasm32-wasip2/release/$3.wasm" "$root/$2/plugin.wasm"
}

cd "$root"
build diffr-plugin-context plugins/context diffr_plugin_context
build diffr-plugin-hide-files plugins/hide-files diffr_plugin_hide_files
build diffr-plugin-deleted-bodies plugins/deleted-bodies diffr_plugin_deleted_bodies
build diffr-plugin-test-bodies plugins/test-bodies diffr_plugin_test_bodies
build diffr-plugin-removed-runs plugins/removed-runs diffr_plugin_removed_runs
build diffr-plugin-group plugins/group diffr_plugin_group
build fixtures-plugin examples/plugins/fixtures fixtures_plugin
