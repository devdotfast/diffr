//! Data types summarising the result of diffing content.

use std::fmt::Display;

use crate::display::hunks::Hunk;
use crate::parse::folds::Fold;
use crate::parse::guess_language::{self, language_name};
use crate::parse::syntax::MatchedPos;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FileContent {
    Text(String),
    Binary,
}

#[derive(Debug, Clone)]
pub(crate) enum FileFormat {
    SupportedLanguage(guess_language::Language),
    PlainText,
    /// A file in a supported language diffed by line: `cause` says why, and
    /// `reason` says so in prose, with the numbers.
    TextFallback {
        cause: FallbackCause,
        reason: String,
    },
    Binary,
}

/// Why a file in a supported language was diffed by line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FallbackCause {
    /// A side is larger than the byte limit.
    ByteLimit,
    /// The AST matching graph grew past the graph limit.
    GraphLimit,
    /// A side has more parse errors than the parse error limit.
    ParseErrorLimit,
    /// The file is tagged `generated`, and so is never parsed.
    Generated,
}

impl Display for FileFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SupportedLanguage(language) => write!(f, "{}", language_name(*language)),
            Self::PlainText => write!(f, "Text"),
            Self::TextFallback { reason, .. } => write!(f, "Text ({})", reason),
            Self::Binary => write!(f, "Binary"),
        }
    }
}

#[derive(Debug)]
pub(crate) struct DiffResult {
    pub(crate) display_path: String,
    /// Additional information to display about this file, such as
    /// "Renamed from x.js to y.js".
    pub(crate) extra_info: Option<String>,

    pub(crate) file_format: FileFormat,
    pub(crate) lhs_src: FileContent,
    pub(crate) rhs_src: FileContent,
    /// Complete selected rows, including ordinary padding and syntax context.
    pub(crate) hunks: Vec<Hunk>,
    pub(crate) lhs_folds: Vec<Fold>,
    pub(crate) rhs_folds: Vec<Fold>,

    pub(crate) lhs_positions: Vec<MatchedPos>,
    pub(crate) rhs_positions: Vec<MatchedPos>,

    /// If the two files do not have exactly the same bytes, the
    /// number of bytes in each file.
    pub(crate) has_byte_changes: Option<(usize, usize)>,
    pub(crate) has_syntactic_changes: bool,
}

impl DiffResult {
    pub(crate) fn has_reportable_change(&self) -> bool {
        if matches!(self.lhs_src, FileContent::Binary)
            || matches!(self.rhs_src, FileContent::Binary)
        {
            return self.has_byte_changes.is_some();
        }

        self.has_syntactic_changes
    }
}
