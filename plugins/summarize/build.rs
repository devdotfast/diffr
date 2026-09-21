fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    println!(
        "cargo::metadata=assets={}",
        std::env::var("CARGO_MANIFEST_DIR").unwrap()
    );
}
