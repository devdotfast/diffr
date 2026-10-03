//! Generated WASM plugin bindings for both kinds of plugin. A shape plugin
//! implements [`GuestPlugin`] and reads and edits the host's trees through the
//! borrowed [`Cursor`]; the classifier implements [`GuestClassifier`]. Both
//! may read the repository through [`git`].
pub mod bindings {
    wit_bindgen::generate!({
        path: "wit",
        world: "diffr-plugin",
        pub_export_macro: true,
        default_bindings_module: "diffr_plugin_sdk::bindings",
        additional_derives: [PartialEq, Eq],
    });
}

/// The classifier's world. Shared records and `git` are the ones above.
pub mod classifier {
    wit_bindgen::generate!({
        path: "wit",
        world: "diffr-classifier",
        pub_export_macro: true,
        export_macro_name: "export_classifier",
        default_bindings_module: "diffr_plugin_sdk::classifier",
        with: {
            "diffr:plugin/types@0.3.0": crate::bindings::diffr::plugin::types,
            "diffr:plugin/git@0.3.0": crate::bindings::diffr::plugin::git,
        },
    });
}

pub mod error;
pub use bindings::diffr::plugin::git;
pub use bindings::diffr::plugin::host::Cursor;
pub use bindings::diffr::plugin::types::{
    Attribute, FileEntry, FileRef, FileSides, FileStatus, Kind, MoveError, NodeView, Region,
    RegionIds, RegionView, Side, Tag, Visit,
};
pub use bindings::exports::diffr::plugin::api::{Guest, GuestPlugin};
pub use classifier::exports::diffr::plugin::classify::{
    Classification, Guest as ClassifierGuest, GuestClassifier,
};

/// The id that names the file itself rather than a region.
pub const ROOT: u32 = 0;

/// Common imports for shape plugins.
pub mod prelude {
    pub use crate::bindings::export;
    pub use crate::{
        Cursor, FileEntry, FileSides, FileStatus, Guest, GuestPlugin, Kind, NodeView, Region,
        RegionIds, RegionView, Side, Visit, ROOT,
    };
}
