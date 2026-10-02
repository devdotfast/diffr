//! diffr's structural diff engine and the diff session around it: parse both
//! sides of a file with tree-sitter, match their syntax trees, find folds
//! with the plugins' fold queries, project the result into the wire records
//! of [`protocol`], and shape each file's record with the plugin pipeline
//! ([`plugin`]) under the configuration ([`config`]) and the file's tags
//! ([`tags`]). Nothing here runs git, spawns threads or runs WASM
//! components: those come from the caller (see [`plugin::Environment`]),
//! so this crate also builds for `wasm32-unknown-unknown`. Files are read
//! only for plugin folders and query files a configuration names on disk.

// I frequently develop difftastic on a newer rustc than the MSRV, so
// these two aren't relevant.
#![allow(renamed_and_removed_lints)]
// This tends to trigger on larger tuples of simple types, and naming
// them would probably be worse for readability.
#![allow(clippy::type_complexity)]
// == "" is often clearer when dealing with strings.
#![allow(clippy::comparison_to_empty)]
// It's common to have pairs foo_lhs and foo_rhs, leading to double
// the number of arguments and triggering this lint.
#![allow(clippy::too_many_arguments)]
// Has false positives on else if chains that sometimes have the same
// body for readability.
#![allow(clippy::if_same_then_else)]
// Good practice in general, but a necessary evil for Syntax. Its Hash
// implementation does not consider the mutable fields, so it is still
// correct.
#![allow(clippy::mutable_key_type)]
// manual_unwrap_or_default was added in Rust 1.79, so earlier versions of
// clippy complain about allowing it.
#![allow(unknown_lints)]
// It's sometimes more readable to explicitly create a vec than to use
// the Default trait.
#![allow(clippy::manual_unwrap_or_default)]
// I find the explicit arithmetic clearer sometimes.
#![allow(clippy::implicit_saturating_sub)]
// It's helpful being super explicit about byte length versus Unicode
// character point length sometimes.
#![allow(clippy::needless_as_bytes)]
// .to_owned() is more explicit on string references.
#![warn(clippy::str_to_string)]
// .to_string() on a String is clearer as .clone().
#![warn(clippy::string_to_string)]
// Debugging features shouldn't be in checked-in code.
#![warn(clippy::todo)]
#![warn(clippy::dbg_macro)]

pub mod config;
pub mod constants;
#[cfg(test)]
mod core_tests;
pub mod diff;
pub mod engine;
pub mod hash;
pub mod line_layout;
pub mod line_parser;
pub mod lines;
pub mod pairing;
pub mod params;
pub mod parse;
pub mod plugin;
pub mod protocol;
pub mod summary;
pub mod tags;
pub mod words;

#[cfg(test)]
use parse::syntax;

#[macro_use]
extern crate log;
