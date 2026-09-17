//! The diffr plugin contract in Rust. `wit/plugin.wit` in the diffr
//! repository is the contract; this crate is its one Rust form, shared by
//! every plugin whether diffr compiles it in or runs it as a WASM component.
//!
//! - [`types`] holds the contract's records: the file entry, each side's
//!   flat preorder region list with parent ids and text, and the moves. They
//!   are generated from `wit/plugin.wit` itself, so there is one definition
//!   of each, and a plugin hands diffr the same records natively and as a
//!   component.
//! - [`Plugin`] is the one trait every plugin implements: `new`, which makes
//!   it from its options, `queries`, then `classify` and `mutate`, taking
//!   and returning exactly those records.
//! - [`host`] holds what diffr gives every plugin: `git`.
//! - [`export!`] makes a plugin the `plugin` resource a component exports
//!   when the crate is built for `wasm32-wasip2`, and
//!   otherwise exposes its native registration for the host to collect.
//!   The same source builds both ways.
//!
//! Plugins reason about regions with the same code:
//!
//! - [`tree`] rebuilds a side's list as a tree ([`tree::sides`], which the
//!   SDK calls on the way in so a plugin is handed [`Pairing`] of [`Source`]
//!   rather than the flat records) and holds the helpers for reading trees ([`walk`], [`OtherSide`], [`one_sided`],
//!   [`docstring_of`], and the rest).
//! - [`apply`] carries moves out. diffr carries every plugin's moves out with
//!   it, so [`Draft`], which carries a plugin's moves out on a copy as it
//!   makes them, and [`apply::Fresh`], which predicts fresh ids, give a
//!   plugin exactly the ids diffr will.
pub mod apply;
pub mod draft;
pub mod host;
#[cfg(not(target_arch = "wasm32"))]
pub mod native;
pub mod tree;
pub mod types;

pub use anyhow;
pub use draft::Draft;
use serde::de::DeserializeOwned;
pub use tree::{
    before_and_after_ids, docstring_of, has_tag, is_fold, line_count, one_sided, path_to,
    siblings_of, sides_with_other_ids, walk, walk_mut, Node, OtherSide, Pairing, Region, Source,
};
pub use types::{
    FileEntry, FileRef, FileSides, FileStatus, Move, Position, QuerySource, Range, Side, Span,
    Visibility, ROOT,
};

/// A diffr plugin: the `plugin` resource of `wit/plugin.wit`. diffr makes one
/// with [`Plugin::new`] when it builds its pipeline, before any file, and
/// calls that one instance for every file of the run.
pub trait Plugin: Sized {
    /// The plugin's options, deserialized from its entry in
    /// `[plugins.<name>]`: a JSON object, validated against the options
    /// schema in `plugin.toml` and filled with its defaults.
    type Options: DeserializeOwned;

    /// Make the plugin from its options. An error, like options that do not
    /// deserialize, is a setup error naming the plugin.
    fn new(options: Self::Options) -> anyhow::Result<Self>;

    /// Named query text, collected once during setup and compiled by diffr.
    fn queries(&self) -> anyhow::Result<Vec<QuerySource>> {
        Ok(Vec::new())
    }

    /// Tags to add to the file's manifest entry before it is diffed. A
    /// plugin that does not classify returns none.
    fn classify(&self, file: &FileEntry) -> anyhow::Result<Vec<String>>;

    /// The moves that shape how the diffed file starts out. `sides` are the
    /// sides the file has, already rebuilt as trees.
    fn mutate(&self, file: &FileEntry, sides: &Pairing<Source>) -> anyhow::Result<Vec<Move>>;
}

/// The contract generated from `wit/plugin.wit`. Its records are plain Rust
/// and compile for every target, so [`types`] re-exports them and a plugin
/// works with the generated records wherever it runs; only the `export!`
/// macro this generates is wasm-specific, and [`export!`] calls it there.
#[doc(hidden)]
pub mod bindings {
    wit_bindgen::generate!({
        path: "../../wit",
        world: "plugin",
        pub_export_macro: true,
        default_bindings_module: "diffr_plugin_sdk::bindings",
        additional_derives: [PartialEq, Eq],
    });
}

#[cfg(target_arch = "wasm32")]
#[doc(hidden)]
pub mod guest;

/// Export a [`Plugin`] as the component's `plugin` resource when the crate
/// is built for `wasm32`: the resource's `new` deserializes the options
/// string into [`Plugin::Options`] and calls [`Plugin::new`], and its
/// `classify` and `mutate` call the instance. Built for anything else it
/// exposes its name and constructor as `DIFFR_PLUGIN` for the host registry.
#[macro_export]
macro_rules! export {
    ($name:literal, $plugin:ty) => {
        #[cfg(not(target_arch = "wasm32"))]
        #[doc(hidden)]
        pub static DIFFR_PLUGIN: $crate::native::Registration = $crate::native::Registration {
            name: $name,
            create: $crate::native::create::<$plugin>,
        };
        #[cfg(target_arch = "wasm32")]
        const _: () = {
            struct DiffrPluginExport;

            impl $crate::bindings::exports::diffr::plugin::guest::Guest for DiffrPluginExport {
                type Plugin = $crate::guest::Instance<$plugin>;
            }

    $crate::bindings::export!(DiffrPluginExport with_types_in $crate::bindings);
        };
    };
}
