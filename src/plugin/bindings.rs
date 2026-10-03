//! The host's side of the plugin contract, generated from the SDK's WIT.
//! A `cursor` resource is the host's [`Cursor`](super::cursor::Cursor).
wasmtime::component::bindgen!({
    path: "crates/diffr-plugin-sdk/wit/plugin.wit",
    world: "diffr-plugin",
    with: { "diffr:plugin/host.cursor": super::cursor::Cursor },
    imports: { default: trappable },
    exports: { default: async | store },
    additional_derives: [PartialEq, Eq],
});

pub(crate) use diffr::plugin::types;

/// The classifier's world.
pub(crate) mod classifier {
    wasmtime::component::bindgen!({
        path: "crates/diffr-plugin-sdk/wit/plugin.wit",
        world: "diffr-classifier",
        with: {
            "diffr:plugin/types": super::diffr::plugin::types,
            "diffr:plugin/git": super::diffr::plugin::git,
        },
        imports: { default: trappable },
        exports: { default: async | store },
    });
}
