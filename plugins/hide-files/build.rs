//! Tells the diffr CLI where this plugin's `plugin.toml`, `queries/` and
//! `plugin.wasm` live, whether the crate comes from the workspace or from
//! the registry. The CLI's build script reads it as
//! `DEP_<links>_ASSETS`.
fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!(
        "cargo::metadata=assets={}",
        std::env::var("CARGO_MANIFEST_DIR").unwrap()
    );
}
