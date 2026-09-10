//! Experimental Git-ref CLI and fixture output adapters.
//! Parsing, fold projection, and hunk context live in the core modules.
pub(crate) mod cli;
mod render;
#[cfg(test)]
mod tests;
mod wire;
