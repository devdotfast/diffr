//! The records of `wit/plugin.wit`'s `types` interface. They are generated
//! from the WIT itself by [`crate::bindings`], so the contract has one
//! definition of each record and nothing to keep in step: a plugin receives
//! and returns exactly these, natively and in a component.
//!
//! This module adds only what the WIT cannot say: [`ROOT`], the id that names
//! the file rather than a region, and the impls below.
pub use crate::bindings::diffr::plugin::types::{
    Cut, FileEntry, FileRef, FileSides, FileStatus, Kind, Leaf, Move, Position, QuerySource, Range,
    Region, Side, Source, SourceSides, Span, Visibility,
};

use crate::tree::Pairing;

/// The id that names the file itself rather than a region.
pub const ROOT: u32 = 0;

impl FileEntry {
    /// The sides the file has, as the pairing the rest of the SDK speaks.
    pub fn sides(&self) -> Pairing<&FileRef> {
        match &self.file {
            FileSides::Both((lhs, rhs)) => Pairing::Both { lhs, rhs },
            FileSides::LeftOnly(lhs) => Pairing::LeftOnly { lhs },
            FileSides::RightOnly(rhs) => Pairing::RightOnly { rhs },
        }
    }

    /// The side the file is named and shown by: the after side, or the
    /// before side for a deleted file.
    pub fn side(&self) -> &FileRef {
        match &self.file {
            FileSides::Both((_, rhs)) | FileSides::RightOnly(rhs) => rhs,
            FileSides::LeftOnly(lhs) => lhs,
        }
    }

    /// [`FileEntry::side`]'s path, the path to show the file under.
    pub fn path(&self) -> &str {
        &self.side().path
    }
}

impl Range {
    /// The lines this range touches, half-open. A range ending at column
    /// zero does not touch its end line.
    pub fn lines(&self) -> std::ops::Range<u32> {
        let end = if self.end.column == 0 {
            self.end.line
        } else {
            self.end.line + 1
        };
        self.start.line..end
    }
}

/// An open region with no label, the state every region starts in.
impl Default for Visibility {
    fn default() -> Self {
        Self {
            collapsed: false,
            label: String::new(),
        }
    }
}
