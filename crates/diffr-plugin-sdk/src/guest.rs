//! The component side of [`crate::export!`]: the `plugin` resource over a
//! [`Plugin`]. The generated guest bindings' records are the contract's
//! records ([`crate::types`] re-exports them), so a call hands the plugin
//! what it was given and returns what the plugin returned.
use crate::bindings::exports::diffr::plugin::guest;
use crate::types::{FileEntry, Move, Source};
use crate::Plugin;

/// One instance of the plugin `P`, the component's `plugin` resource.
pub struct Instance<P>(P);

impl<P: Plugin + 'static> guest::GuestPlugin for Instance<P> {
    fn new(options: String) -> Result<guest::Plugin, String> {
        let options: P::Options =
            serde_json::from_str(&options).map_err(|error| format!("invalid options: {error}"))?;
        let plugin = P::new(options).map_err(|error| format!("{error:#}"))?;
        Ok(guest::Plugin::new(Instance(plugin)))
    }

    fn classify(&self, file: FileEntry) -> Result<Vec<String>, String> {
        self.0.classify(&file).map_err(|error| format!("{error:#}"))
    }

    fn mutate(
        &self,
        file: FileEntry,
        lhs: Option<Source>,
        rhs: Option<Source>,
    ) -> Result<Vec<Move>, String> {
        self.0
            .mutate(&file, lhs.as_ref(), rhs.as_ref())
            .map_err(|error| format!("{error:#}"))
    }
}
