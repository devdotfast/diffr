//! The wire records ([`diffr_core::protocol`]), one file's record, and the
//! stream that writes them.
pub use diffr_core::protocol::*;
pub mod record;
#[cfg(not(target_family = "wasm"))]
pub mod stream;
