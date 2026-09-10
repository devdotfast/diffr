//! Experimental Git-ref CLI and fixture output adapters.
//! Parsing, fold projection, and hunk context live in the core modules.
pub(crate) mod cli;
mod render;
#[cfg(test)]
mod tests;
mod wire;

use crate::config::Params;
use crate::summary::DiffResult;

impl DiffResult {
    #[cfg(test)]
    pub(crate) fn from_sources(path: &str, lhs: &str, rhs: &str) -> Self {
        Self::from_sources_with_params(path, lhs, rhs, &Params::default())
    }

    pub(crate) fn from_sources_with_params(
        path: &str,
        lhs: &str,
        rhs: &str,
        params: &Params,
    ) -> Self {
        let file = crate::options::FileArgument::NamedPath(path.into());
        crate::diff_file_content(
            params,
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
