//! The component side of [`crate::export!`]: the `plugin` resource over a
//! [`Plugin`]. The generated guest bindings' records are the contract's
//! records ([`crate::types`] re-exports them), so a call hands the plugin
//! what it was given and returns what the plugin returned; the sides are
//! rebuilt as trees on the way in, the one conversion the SDK makes.
use crate::bindings::exports::diffr::plugin::guest;
use crate::types::{FileEntry, Move, SourceSides};
use crate::{tree, Plugin};

/// One instance of the plugin `P`, the component's `plugin` resource.
pub struct Instance<P>(P);

impl<P: Plugin + 'static> guest::GuestPlugin for Instance<P> {
    fn new(options: String) -> Result<guest::Plugin, String> {
        let options: P::Options =
            serde_json::from_str(&options).map_err(|error| format!("invalid options: {error}"))?;
        let plugin = P::new(options).map_err(|error| format!("{error:#}"))?;
        Ok(guest::Plugin::new(Instance(plugin)))
    }

    fn enrich(
        &self,
        file: FileEntry,
        sides: SourceSides,
    ) -> Result<Vec<crate::Annotation>, String> {
        let sides = tree::sides(&sides).map_err(|error| format!("{error:#}"))?;
        self.0
            .enrich(&file, &sides)
            .map_err(|error| format!("{error:#}"))
    }
    fn queries(&self) -> Result<Vec<crate::QuerySource>, String> {
        self.0.queries().map_err(|error| format!("{error:#}"))
    }

    fn classify(&self, file: FileEntry) -> Result<Vec<String>, String> {
        self.0.classify(&file).map_err(|error| format!("{error:#}"))
    }

    fn mutate(&self, file: FileEntry, sides: SourceSides) -> Result<Vec<Move>, String> {
        let sides = tree::sides(&sides).map_err(|error| format!("{error:#}"))?;
        self.0
            .mutate(&file, &sides)
            .map_err(|error| format!("{error:#}"))
    }
}
