//! The records of `wit/plugin.wit`'s `types` interface. They are generated
//! from the WIT itself by [`crate::bindings`], so the contract has one
//! definition of each record and nothing to keep in step: a plugin receives
//! and returns exactly these, natively and in a component.
//!
//! This module adds only what the WIT cannot say: [`ROOT`], the id that names
//! the file rather than a region, and the two impls below.
pub use crate::bindings::diffr::plugin::types::{
    Cut, FileEntry, FileStatus, Kind, Leaf, Move, Position, Range, Region, Side, Source, Span,
    Visibility,
};

/// The id that names the file itself rather than a region.
pub const ROOT: u32 = 0;

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
