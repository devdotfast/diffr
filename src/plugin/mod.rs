//! The plugin host: plugins decide how each diffed file is shown. Nothing in
//! this module is specific to one plugin; the bundled plugins live in
//! `plugins/`.
//!
//! Every plugin is a WebAssembly component implementing the SDK's WIT contract.
//! [`config`] resolves its manifest, options and component; [`Pipeline`] loads
//! the enabled components and runs them in order on each file.
pub(crate) mod bindings;
pub(crate) mod builtin;
pub(crate) mod classify;
pub(crate) mod config;
pub(crate) mod cursor;
pub(crate) mod queries;
pub(crate) mod wasm;

pub(crate) use classify::Classifier;
pub(crate) use wasm::Pipeline;

#[cfg(test)]
mod tests;

use crate::pairing::Pairing;
use crate::protocol::{self, FileChange, FileStatus};
use bindings::types;
use std::fmt;

/// A plugin stopped the run: its own failure, or a move it asked for
/// that could not be carried out. The stream reports `mutation_failed`.
#[derive(Debug)]
pub(crate) struct MutationFailed(pub(crate) String);

impl fmt::Display for MutationFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mutation {}", self.0)
    }
}

impl std::error::Error for MutationFailed {}

/// The contract's record of a manifest entry.
pub(crate) fn file_entry(file: &FileChange) -> types::FileEntry {
    let file_ref = |side: &protocol::FileRef| types::FileRef {
        path: side.path.clone(),
        oid: side.oid.clone(),
        mode: side.mode.clone(),
    };
    types::FileEntry {
        file: match &file.file {
            Pairing::Both { lhs, rhs } => types::FileSides::Both((file_ref(lhs), file_ref(rhs))),
            Pairing::LeftOnly { lhs } => types::FileSides::LeftOnly(file_ref(lhs)),
            Pairing::RightOnly { rhs } => types::FileSides::RightOnly(file_ref(rhs)),
        },
        status: match file.status {
            FileStatus::Added => types::FileStatus::Added,
            FileStatus::Deleted => types::FileStatus::Deleted,
            FileStatus::Modified => types::FileStatus::Modified,
            FileStatus::Renamed => types::FileStatus::Renamed,
            FileStatus::Copied => types::FileStatus::Copied,
            FileStatus::TypeChanged => types::FileStatus::TypeChanged,
        },
        tags: file.tags.clone(),
    }
}
