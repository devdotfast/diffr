//! Experimental Git-ref CLI and fixture output adapters.
//! Parsing, fold projection, and hunk context live in the core modules.
pub(crate) mod cli;
mod render;
mod wire;

use crate::summary::DiffResult;

impl DiffResult {
    pub(crate) fn from_sources(path: &str, lhs: &str, rhs: &str) -> Self {
        let file = crate::options::FileArgument::NamedPath(path.into());
        crate::diff_file_content(
            path,
            None,
            &file,
            &file,
            lhs,
            rhs,
            &crate::options::DisplayOptions::default(),
            &crate::options::DiffOptions::default(),
            &[],
        )
    }
}
