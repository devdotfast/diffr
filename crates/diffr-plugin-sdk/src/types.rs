//! The records of `wit/plugin.wit`'s `types` interface, as plain Rust. A
//! plugin receives and returns exactly these, natively and in a component;
//! in a component the export macro converts them to and from the generated
//! guest bindings.

/// The before (`Lhs`) or after (`Rhs`) side of a comparison.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Lhs,
    Rhs,
}

/// Git's delta status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
    TypeChanged,
}

/// One changed file, as its manifest entry describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// The path on the after side, or on the before side when the file was
    /// deleted.
    pub path: String,
    /// The path on the before side, when the file has one.
    pub old_path: Option<String>,
    pub status: FileStatus,
    /// Sorted and deduplicated. During `classify`, the tags so far.
    pub tags: Vec<String>,
}

/// 0-based line, and 0-based byte column in the UTF-8 text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    pub line: u32,
    pub column: u32,
}

/// Half-open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub start: Position,
    pub end: Position,
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

/// Bytes painted as changed on one line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub line: u32,
    pub start_column: u32,
    pub end_column: u32,
}

/// How a region starts out: collapsed or open, and the label shown while it
/// is collapsed (empty for none).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Visibility {
    pub collapsed: bool,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leaf {
    /// Row alignment: the leaf on the other side with the same value lines
    /// up with this one.
    pub alignment_id: u32,
    pub changed: Vec<Span>,
}

/// A leaf tiles the file; a fold's range is the hull of its children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Leaf(Leaf),
    Fold,
}

/// One region of a side's tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Region {
    /// Unique across both sides of the file, never [`ROOT`].
    pub id: u32,
    /// The fold that holds this region, or [`ROOT`] for a top-level region.
    pub parent: u32,
    /// Regions sharing it open and close together, on either side.
    pub fold_state_id: u32,
    pub range: Range,
    /// The tags the fold queries set, `<plugin>:<name>`.
    pub tags: Vec<String>,
    pub visibility: Visibility,
    pub kind: Kind,
}

/// One side of a diffed file. `regions` lists the tree in preorder: a fold
/// comes before its children, and siblings keep their order. A binary file's
/// sides have empty text and no regions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub text: String,
    pub regions: Vec<Region>,
}

/// The id that names the file itself rather than a region.
pub const ROOT: u32 = 0;

/// What a plugin asks for. A region is named by its id, or the file by
/// [`ROOT`] where a move allows it; `wit/plugin.wit` documents each move and
/// how fresh ids are handed out, and [`crate::apply`] carries them out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Move {
    /// Split a leaf, and the leaf paired with it, at a line offset relative
    /// to its first line.
    Cut { region: u32, at: u32 },
    /// Wrap two or more consecutive siblings on each side that holds them in
    /// a new open, unlabelled fold without tags.
    JoinFolds { regions: Vec<u32> },
    /// Every region in the listed regions' fold states takes the first
    /// region's fold state id and collapsed state.
    LinkFoldState { regions: Vec<u32> },
    /// Collapse or open every region sharing the region's fold state, or
    /// hide or show the file.
    SetCollapsed { region: u32, collapsed: bool },
    /// Set or clear the label of one region, or of the file.
    SetLabel { region: u32, label: Option<String> },
    /// Replace one region's tags.
    SetTags { region: u32, tags: Vec<String> },
}
