//! What the CLI adds to the plugin pipeline ([`diffr_core::plugin`]):
//! component plugins run in Wasmtime ([`wasm`]), and the environment its
//! plugins reach git, components and the clock through ([`system`]).
pub use diffr_core::plugin::*;
pub mod system;
pub mod wasm;

#[cfg(test)]
mod tests;
