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
//! read-write as `.`, the environment inherited, and the network open. What
//! it writes to stdout or stderr diffr writes to its own stderr, a line at a
//! time and with the plugin's name in front: diffr's stdout is the stream,
//! and a plugin has no logging call of its own, it just prints.
use super::config::ComponentSource;
use super::host::Host;
use super::Runner;
use anyhow::Context as _;
use bytes::Bytes;
use diffr_plugin_sdk::types as contract;
use std::io::Write as _;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;
use wasmtime::component::{Component, HasSelf, Linker, ResourceAny, ResourceTable};
use wasmtime::{Cache, CacheConfig, Config, Engine, Store};
use wasmtime_wasi::p2::{
    IoView, OutputStream, Pollable, StdoutStream, StreamError, StreamResult, WasiCtx,
    WasiCtxBuilder, WasiView,
};
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
}

/// One of a guest's output streams, stdout or stderr, and what it has
/// written since its last newline.
struct Pending {
    name: Arc<str>,
    line: Vec<u8>,
}

impl Pending {
    /// Write one line to diffr's stderr with the plugin's name in front.
    fn write_line(&self, line: &[u8]) -> std::io::Result<()> {
        let line = String::from_utf8_lossy(line);
        writeln!(std::io::stderr().lock(), "[{}] {line}", self.name)
    }
}

/// Whatever a guest wrote without ending the line still reaches diffr's
/// stderr, when the store the streams belong to is dropped.
impl Drop for Pending {
    fn drop(&mut self) {
        if !self.line.is_empty() {
            let line = std::mem::take(&mut self.line);
            let _ = self.write_line(&line);
        }
    }
}

/// A guest's stdout or stderr, written to diffr's stderr a line at a time,
/// each line prefixed with `[<plugin name>] `. Every stream WASI makes of it
/// shares the one unfinished line, so a write that does not end a line waits
/// for the rest of it.
#[derive(Clone)]
struct Prefixed(Arc<Mutex<Pending>>);

impl Prefixed {
    fn new(name: Arc<str>) -> Self {
        Self(Arc::new(Mutex::new(Pending {
            name,
            line: Vec::new(),
        })))
    }
}

impl StdoutStream for Prefixed {
    fn stream(&self) -> Box<dyn OutputStream> {
        Box::new(self.clone())
    }

    fn isatty(&self) -> bool {
        false
    }
}

#[wasmtime_wasi::async_trait]
impl Pollable for Prefixed {
    async fn ready(&mut self) {}
}

impl OutputStream for Prefixed {
    fn write(&mut self, bytes: Bytes) -> StreamResult<()> {
        let mut pending = self
            .0
            .lock()
            .expect("no write panics while it holds a guest's output");
        pending.line.extend_from_slice(&bytes);
        while let Some(end) = pending.line.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = pending.line.drain(..=end).take(end).collect();
            pending
                .write_line(&line)
                .map_err(|error| StreamError::LastOperationFailed(error.into()))?;
        }
        Ok(())
    }

    fn flush(&mut self) -> StreamResult<()> {
        std::io::stderr()
            .flush()
            .map_err(|error| StreamError::LastOperationFailed(error.into()))
    }

    fn check_write(&mut self) -> StreamResult<usize> {
        Ok(1024 * 1024)
    }
}

/// A compiled, linked component.
pub(crate) struct WasmPlugin {
    engine: Engine,
    pre: bindings::PluginPre<State>,
}

impl WasmPlugin {
    /// Compile and link an external or bundled component.
    pub(crate) fn load(engine: &Engine, source: &ComponentSource) -> anyhow::Result<Self> {
        let started = Instant::now();
        let (component, label) = match source {
            ComponentSource::File(path) => (
                Component::from_file(engine, path),
                path.display().to_string(),
            ),
            ComponentSource::Bundled(bytes) => {
                (Component::new(engine, bytes), "bundled component".into())
            }
        };
        let component = component.with_context(|| format!("compiling {label}"))?;
        let mut linker = Linker::<State>::new(engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        bindings::Plugin::add_to_linker::<State, HasSelf<State>>(&mut linker, |state| state)?;
        let pre = bindings::PluginPre::new(
            linker
                .instantiate_pre(&component)
                .with_context(|| format!("linking {label}"))?,
        )?;
        log::debug!("compiled and linked {} in {:?}", label, started.elapsed());
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
            .stdout(Prefixed::new(Arc::clone(&host.name)))
            .stderr(Prefixed::new(Arc::clone(&host.name)))
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
    fn queries(&self, host: Host) -> anyhow::Result<Vec<contract::QuerySource>> {
        let instance = &mut *self.enter(host);
        let sources = instance
            .exports
            .diffr_plugin_guest()
            .plugin()
            .call_queries(&mut instance.store, instance.plugin)?
            .map_err(anyhow::Error::msg)?;
        Ok(sources
            .into_iter()
            .map(|source| contract::QuerySource {
                language: source.language,
                name: source.name,
                text: source.text,
            })
            .collect())
    }

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
        sides: &contract::SourceSides,
    ) -> anyhow::Result<Vec<contract::Move>> {
        let sides = source_sides(sides);
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
                &sides,
            )?
            .map_err(anyhow::Error::msg)?;
        log::debug!("called in {:?}", started.elapsed());
        Ok(moves.into_iter().map(lift).collect())
    }
}

fn file_entry(file: &contract::FileEntry) -> types::FileEntry {
    let file_ref = |side: &contract::FileRef| types::FileRef {
        path: side.path.clone(),
        oid: side.oid.clone(),
        mode: side.mode.clone(),
    };
    types::FileEntry {
        file: match &file.file {
            contract::FileSides::Both((lhs, rhs)) => {
                types::FileSides::Both((file_ref(lhs), file_ref(rhs)))
            }
            contract::FileSides::LeftOnly(lhs) => types::FileSides::LeftOnly(file_ref(lhs)),
            contract::FileSides::RightOnly(rhs) => types::FileSides::RightOnly(file_ref(rhs)),
        },
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

fn source_sides(sides: &contract::SourceSides) -> types::SourceSides {
    match sides {
        contract::SourceSides::Both((lhs, rhs)) => {
            types::SourceSides::Both((source(lhs), source(rhs)))
        }
        contract::SourceSides::LeftOnly(lhs) => types::SourceSides::LeftOnly(source(lhs)),
        contract::SourceSides::RightOnly(rhs) => types::SourceSides::RightOnly(source(rhs)),
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
