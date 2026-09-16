//! The WASM runner: a folder's `plugin.wasm`, a component of the `plugin`
//! world in `wit/plugin.wit`, run through wasmtime.
//!
//! Each component is compiled once, when the pipeline is built (with
//! wasmtime's disk cache, so an unchanged component is not recompiled on the
//! next run), linked against WASI and the `host` imports, and instantiated
//! once in its own [`Store`]. The plugin's `new` makes the one `plugin`
//! resource of the run in that instance, and every `classify` and `mutate`
//! calls that resource. A store runs one call at a time, so it sits behind a
//! mutex: the rayon workers call one component plugin one file at a time.
//! The records a call gets are the ones a native plugin gets, lowered field
//! for field into the generated bindings.
//!
//! A plugin runs with full access: WASI with the working directory preopened
//! read-write as `.`, the environment inherited, and the network open. Its
//! stdout goes to diffr's stderr, since diffr's stdout is the stream.
use super::host::Host;
use super::Runner;
use anyhow::Context as _;
use diffr_plugin_sdk::types as contract;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};
use std::time::Instant;
use wasmtime::component::{Component, HasSelf, Linker, ResourceAny, ResourceTable};
use wasmtime::{Cache, CacheConfig, Config, Engine, Store};
use wasmtime_wasi::p2::{IoView, WasiCtx, WasiCtxBuilder, WasiView};
use wasmtime_wasi::{DirPerms, FilePerms};

mod bindings {
    wasmtime::component::bindgen!({
        path: "wit/plugin.wit",
        world: "plugin",
        trappable_imports: true,
    });
}

use bindings::diffr::plugin::{host, types};

/// The engine every component of one pipeline compiles with.
pub(crate) fn engine() -> anyhow::Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.cache(Some(
        Cache::new(CacheConfig::new()).context("wasmtime's compilation cache")?,
    ));
    Engine::new(&config)
}

/// What a plugin instance's store holds. `host` is the host of the call in
/// progress, set before each call.
struct State {
    wasi: WasiCtx,
    table: ResourceTable,
    host: Host,
}

impl IoView for State {
    fn table(&mut self) -> &mut ResourceTable {
        &mut self.table
    }
}

impl WasiView for State {
    fn ctx(&mut self) -> &mut WasiCtx {
        &mut self.wasi
    }
}

impl types::Host for State {}

impl host::Host for State {
    /// A failure of the host itself reaches the plugin as the call's error,
    /// as it does for a native plugin, rather than trapping.
    fn git(&mut self, args: Vec<String>) -> wasmtime::Result<Result<String, String>> {
        Ok(self
            .host
            .git(&args)
            .unwrap_or_else(|error| Err(format!("{error:#}"))))
    }

    fn log(&mut self, message: String) -> wasmtime::Result<()> {
        self.host.log(&message);
        Ok(())
    }
}

/// A compiled, linked component.
pub(crate) struct WasmPlugin {
    engine: Engine,
    pre: bindings::PluginPre<State>,
}

impl WasmPlugin {
    /// Compile and link the component at `path`.
    pub(crate) fn load(engine: &Engine, path: &Path) -> anyhow::Result<Self> {
        let started = Instant::now();
        let component = Component::from_file(engine, path)
            .with_context(|| format!("compiling {}", path.display()))?;
        let mut linker = Linker::<State>::new(engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        bindings::Plugin::add_to_linker::<State, HasSelf<State>>(&mut linker, |state| state)?;
        let pre = bindings::PluginPre::new(
            linker
                .instantiate_pre(&component)
                .with_context(|| format!("linking {}", path.display()))?,
        )?;
        log::debug!(
            "compiled and linked {} in {:?}",
            path.display(),
            started.elapsed()
        );
        Ok(Self {
            engine: engine.clone(),
            pre,
        })
    }

    /// Instantiate the component in a store of its own and make the plugin
    /// with its `new`, from `options`.
    pub(crate) fn create(&self, host: Host, options: &str) -> anyhow::Result<Box<dyn Runner>> {
        let started = Instant::now();
        let mut wasi = WasiCtxBuilder::new();
        wasi.inherit_env()
            .stdout(wasmtime_wasi::p2::stderr())
            .inherit_stderr()
            .inherit_network()
            .allow_ip_name_lookup(true)
            .preopened_dir(&*host.workdir, ".", DirPerms::all(), FilePerms::all())
            .with_context(|| format!("preopening {}", host.workdir.display()))?;
        let name = host.name.clone();
        let mut store = Store::new(
            &self.engine,
            State {
                wasi: wasi.build(),
                table: ResourceTable::new(),
                host,
            },
        );
        let exports = self.pre.instantiate(&mut store)?;
        let plugin = exports
            .diffr_plugin_guest()
            .plugin()
            .call_new(&mut store, options)?
            .map_err(anyhow::Error::msg)?;
        log::debug!(
            "plugin {name}: instantiated and made in {:?}",
            started.elapsed()
        );
        Ok(Box::new(WasmInstance(Mutex::new(Instance {
            store,
            exports,
            plugin,
        }))))
    }
}

/// A component's instance and the `plugin` resource `new` made in it.
struct Instance {
    store: Store<State>,
    exports: bindings::Plugin,
    plugin: ResourceAny,
}

/// The instance, one call at a time.
struct WasmInstance(Mutex<Instance>);

impl WasmInstance {
    /// The instance, with `host` behind its imports for the call.
    fn enter(&self, host: Host) -> MutexGuard<'_, Instance> {
        let mut instance = self
            .0
            .lock()
            .expect("no call panics while it holds a plugin's store");
        instance.store.data_mut().host = host;
        instance
    }
}

impl Runner for WasmInstance {
    fn classify(&self, host: Host, file: &contract::FileEntry) -> anyhow::Result<Vec<String>> {
        let instance = &mut *self.enter(host);
        instance
            .exports
            .diffr_plugin_guest()
            .plugin()
            .call_classify(&mut instance.store, instance.plugin, &file_entry(file))?
            .map_err(anyhow::Error::msg)
    }

    fn mutate(
        &self,
        host: Host,
        file: &contract::FileEntry,
        lhs: Option<&contract::Source>,
        rhs: Option<&contract::Source>,
    ) -> anyhow::Result<Vec<contract::Move>> {
        let (lhs, rhs) = (lhs.map(source), rhs.map(source));
        let instance = &mut *self.enter(host);
        let started = Instant::now();
        let moves = instance
            .exports
            .diffr_plugin_guest()
            .plugin()
            .call_mutate(
                &mut instance.store,
                instance.plugin,
                &file_entry(file),
                lhs.as_ref(),
                rhs.as_ref(),
            )?
            .map_err(anyhow::Error::msg)?;
        log::debug!("called in {:?}", started.elapsed());
        Ok(moves.into_iter().map(lift).collect())
    }
}

fn file_entry(file: &contract::FileEntry) -> types::FileEntry {
    types::FileEntry {
        path: file.path.clone(),
        old_path: file.old_path.clone(),
        status: match file.status {
            contract::FileStatus::Added => types::FileStatus::Added,
            contract::FileStatus::Deleted => types::FileStatus::Deleted,
            contract::FileStatus::Modified => types::FileStatus::Modified,
            contract::FileStatus::Renamed => types::FileStatus::Renamed,
            contract::FileStatus::Copied => types::FileStatus::Copied,
            contract::FileStatus::TypeChanged => types::FileStatus::TypeChanged,
        },
        tags: file.tags.clone(),
    }
}

fn source(side: &contract::Source) -> types::Source {
    let position = |position: contract::Position| types::Position {
        line: position.line,
        column: position.column,
    };
    types::Source {
        text: side.text.clone(),
        regions: side
            .regions
            .iter()
            .map(|region| types::Region {
                id: region.id,
                parent: region.parent,
                fold_state_id: region.fold_state_id,
                range: types::Range {
                    start: position(region.range.start),
                    end: position(region.range.end),
                },
                tags: region.tags.clone(),
                visibility: types::Visibility {
                    collapsed: region.visibility.collapsed,
                    label: region.visibility.label.clone(),
                },
                kind: match &region.kind {
                    contract::Kind::Leaf(leaf) => types::Kind::Leaf(types::Leaf {
                        alignment_id: leaf.alignment_id,
                        changed: leaf
                            .changed
                            .iter()
                            .map(|span| types::Span {
                                line: span.line,
                                start_column: span.start_column,
                                end_column: span.end_column,
                            })
                            .collect(),
                    }),
                    contract::Kind::Fold => types::Kind::Fold,
                },
            })
            .collect(),
    }
}

fn lift(next: types::Move) -> contract::Move {
    match next {
        types::Move::Cut(types::Cut { region, at }) => {
            contract::Move::Cut(contract::Cut { region, at })
        }
        types::Move::JoinFolds(regions) => contract::Move::JoinFolds(regions),
        types::Move::LinkFoldState(regions) => contract::Move::LinkFoldState(regions),
        types::Move::SetCollapsed((region, collapsed)) => {
            contract::Move::SetCollapsed((region, collapsed))
        }
        types::Move::SetLabel((region, label)) => contract::Move::SetLabel((region, label)),
        types::Move::SetTags((region, tags)) => contract::Move::SetTags((region, tags)),
    }
}
