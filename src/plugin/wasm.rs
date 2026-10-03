//! Plugin workers: each is a thread holding an instance of every plugin. A
//! file is walked by every plugin on one worker; files interleave while a
//! plugin awaits I/O.
use super::bindings::{
    self,
    classifier::{DiffrClassifier, DiffrClassifierPre},
    exports::diffr::plugin::api::GuestPlugin,
    types::{self, Attribute, FileEntry, MoveError, RegionIds, RegionView, RowSummary, Side, Tag},
    DiffrPlugin, DiffrPluginPre,
};
use super::config::{ComponentSource, Entry};
use super::cursor::Cursor;
use super::{file_entry, MutationFailed};
use crate::pairing::Pairing;
use crate::protocol;
use anyhow::Context as _;
use futures::{stream::FuturesUnordered, StreamExt};
use gix::attrs::StateRef as AttrState;
use gix::bstr::ByteSlice;
use output::Prefixed;
use serde_json::Value;
use std::collections::BTreeSet;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::oneshot;
use wasmtime::component::{
    Accessor, Component as Compiled, HasSelf, Linker, Resource, ResourceAny, ResourceTable,
};
use wasmtime::{Cache, CacheConfig, Config, Engine, Store};
use wasmtime_wasi::{FsPerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

mod output;

/// The engine every component of one pipeline compiles with.
fn engine() -> anyhow::Result<Engine> {
    let mut config = Config::new();
    config.wasm_component_model(true);
    config.wasm_component_model_async(true);
    // The disk cache is an optimization; read-only homes must still run plugins.
    if let Ok(cache) = Cache::new(CacheConfig::new()) {
        config.cache(Some(cache));
    }
    Ok(Engine::new(&config)?)
}

/// One worker's Store, shared by its plugins.
struct State {
    wasi: WasiCtx,
    http: wasmtime_wasi_http::WasiHttpCtx,
    table: ResourceTable,
    workdir: PathBuf,
    /// The repository `git` reads, opened the first time a plugin asks.
    repo: Option<gix::Repository>,
}

impl State {
    fn new(workdir: &Path) -> anyhow::Result<Self> {
        let mut wasi = WasiCtxBuilder::new();
        wasi.inherit_env()
            .stdout(Prefixed::new("plugins".into()))
            .stderr(Prefixed::new("plugins".into()))
            .inherit_network()
            .allow_ip_name_lookup(true)
            .preopened_dir(workdir, ".", FsPerms::ReadWrite)
            .map_err(anyhow::Error::from)
            .with_context(|| format!("preopening {}", workdir.display()))?;
        Ok(Self {
            wasi: wasi.build(),
            http: wasmtime_wasi_http::WasiHttpCtx::new(),
            table: ResourceTable::new(),
            workdir: workdir.into(),
            repo: None,
        })
    }

    /// The repository at the working directory. Opening it fails, and is
    /// reported to the plugin, when the directory is not in one.
    fn repo(&mut self) -> Result<&gix::Repository, String> {
        if self.repo.is_none() {
            let repo = gix::open(&self.workdir)
                .map_err(|error| format!("{}: {error}", self.workdir.display()))?;
            self.repo = Some(repo);
        }
        Ok(self.repo.as_ref().expect("opened above"))
    }
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

impl wasmtime_wasi_http::WasiHttpView for State {
    fn http(&mut self) -> wasmtime_wasi_http::WasiHttpCtxView<'_> {
        wasmtime_wasi_http::WasiHttpCtxView {
            ctx: &mut self.http,
            table: &mut self.table,
            hooks: wasmtime_wasi_http::default_hooks(),
        }
    }
}

impl types::Host for State {}

/// Walk the live tree, calling the plugin on each node before (Pre) and after
/// (Post) its children. False on Pre skips the children and Post; errors stop.
async fn walk(
    accessor: &Accessor<State>,
    plugin: &GuestPlugin<'_>,
    configured: ResourceAny,
    cursor: &Resource<Cursor>,
) -> anyhow::Result<()> {
    let mut after = None;
    while let Some(node) = next_child(accessor, cursor, None, after)? {
        subtree(accessor, plugin, configured, cursor, node).await?;
        after = Some(node);
    }
    Ok(())
}

/// Visit `node` before and after its children.
async fn subtree(
    accessor: &Accessor<State>,
    plugin: &GuestPlugin<'_>,
    configured: ResourceAny,
    cursor: &Resource<Cursor>,
    node: u32,
) -> anyhow::Result<()> {
    let visit = async |phase| -> anyhow::Result<bool> {
        accessor.with(|mut access| -> anyhow::Result<()> {
            access.data_mut().table.get_mut(cursor)?.id = Some(node);
            Ok(())
        })?;
        let borrowed = Resource::new_borrow(cursor.rep());
        plugin
            .call_visit(accessor, configured, borrowed, phase)
            .await?
            .map_err(anyhow::Error::msg)
    };
    if !visit(types::Visit::Pre).await? {
        return Ok(());
    }
    let mut after = None;
    while let Some(child) = next_child(accessor, cursor, Some(node), after)? {
        Box::pin(subtree(accessor, plugin, configured, cursor, child)).await?;
        after = Some(child);
    }
    visit(types::Visit::Post).await?;
    Ok(())
}

fn next_child(
    accessor: &Accessor<State>,
    cursor: &Resource<Cursor>,
    parent: Option<u32>,
    after: Option<u32>,
) -> anyhow::Result<Option<u32>> {
    accessor.with(|mut access| {
        Ok(access
            .data_mut()
            .table
            .get(cursor)?
            .next_child(parent, after)?)
    })
}

impl bindings::diffr::plugin::host::HostCursor for State {
    fn file(&mut self, c: Resource<Cursor>) -> wasmtime::Result<FileEntry> {
        Ok(self.table.get(&c)?.file.clone())
    }
    fn id(&mut self, c: Resource<Cursor>) -> wasmtime::Result<u32> {
        Ok(self
            .table
            .get(&c)?
            .id
            .expect("the cursor is positioned during a callback"))
    }
    fn siblings(
        &mut self,
        c: Resource<Cursor>,
        id: u32,
    ) -> wasmtime::Result<Result<Vec<u32>, MoveError>> {
        Ok(self.table.get(&c)?.siblings(id))
    }
    fn ancestors(
        &mut self,
        c: Resource<Cursor>,
        id: u32,
    ) -> wasmtime::Result<Result<Vec<types::Region>, MoveError>> {
        Ok(self.table.get(&c)?.ancestors(id))
    }
    fn get(
        &mut self,
        c: Resource<Cursor>,
        id: u32,
    ) -> wasmtime::Result<Result<RegionView, MoveError>> {
        Ok(self.table.get(&c)?.get(id))
    }
    fn text(
        &mut self,
        c: Resource<Cursor>,
        id: u32,
    ) -> wasmtime::Result<Result<String, MoveError>> {
        Ok(self.table.get(&c)?.text(id))
    }
    fn display(
        &mut self,
        c: Resource<Cursor>,
        id: u32,
    ) -> wasmtime::Result<Result<RowSummary, MoveError>> {
        Ok(self.table.get(&c)?.display(id))
    }
    fn matching_siblings(
        &mut self,
        c: Resource<Cursor>,
        ids: Vec<u32>,
    ) -> wasmtime::Result<Result<Option<Vec<u32>>, MoveError>> {
        Ok(self.table.get(&c)?.matching_siblings(&ids))
    }
    fn leaves(
        &mut self,
        c: Resource<Cursor>,
        side: Side,
        start: u32,
        end: u32,
    ) -> wasmtime::Result<Vec<u32>> {
        Ok(self.table.get(&c)?.leaves(side, start, end))
    }
    fn has_changes(
        &mut self,
        c: Resource<Cursor>,
        id: u32,
    ) -> wasmtime::Result<Result<bool, MoveError>> {
        Ok(self.table.get(&c)?.has_changes(id))
    }
    fn related(
        &mut self,
        c: Resource<Cursor>,
        id: u32,
        name: String,
    ) -> wasmtime::Result<Result<Vec<u32>, MoveError>> {
        Ok(self.table.get(&c)?.related(id, &name))
    }
    fn paired_leaf(
        &mut self,
        c: Resource<Cursor>,
        id: u32,
    ) -> wasmtime::Result<Result<Option<u32>, MoveError>> {
        Ok(self.table.get(&c)?.paired_leaf(id))
    }
    fn linked_regions(
        &mut self,
        c: Resource<Cursor>,
        id: u32,
    ) -> wasmtime::Result<Result<Vec<u32>, MoveError>> {
        Ok(self.table.get(&c)?.linked_regions(id))
    }
    fn is_one_sided(
        &mut self,
        c: Resource<Cursor>,
        id: u32,
    ) -> wasmtime::Result<Result<bool, MoveError>> {
        Ok(self.table.get(&c)?.is_one_sided(id))
    }
    fn source(&mut self, c: Resource<Cursor>, side: Side) -> wasmtime::Result<Option<String>> {
        Ok(self.table.get(&c)?.source(side))
    }
    fn cut(
        &mut self,
        c: Resource<Cursor>,
        region: u32,
        offset: u32,
    ) -> wasmtime::Result<Result<RegionIds, MoveError>> {
        Ok(self.table.get_mut(&c)?.cut(region, offset))
    }
    fn join(
        &mut self,
        c: Resource<Cursor>,
        regions: Vec<u32>,
    ) -> wasmtime::Result<Result<RegionIds, MoveError>> {
        Ok(self.table.get_mut(&c)?.join(&regions))
    }
    fn link(
        &mut self,
        c: Resource<Cursor>,
        regions: Vec<u32>,
    ) -> wasmtime::Result<Result<(), MoveError>> {
        Ok(self.table.get_mut(&c)?.link(&regions))
    }
    fn set_collapsed(
        &mut self,
        c: Resource<Cursor>,
        region: u32,
        collapsed: bool,
    ) -> wasmtime::Result<Result<(), MoveError>> {
        Ok(self.table.get_mut(&c)?.set_collapsed(region, collapsed))
    }
    fn set_label(
        &mut self,
        c: Resource<Cursor>,
        region: u32,
        label: Option<String>,
    ) -> wasmtime::Result<Result<(), MoveError>> {
        Ok(self.table.get_mut(&c)?.set_label(region, label))
    }
    fn drop(&mut self, c: Resource<Cursor>) -> wasmtime::Result<()> {
        self.table.delete(c)?;
        Ok(())
    }
}

impl bindings::diffr::plugin::host::Host for State {}

impl bindings::diffr::plugin::git::Host for State {
    fn check_attr(
        &mut self,
        attributes: Vec<String>,
        path: String,
    ) -> wasmtime::Result<Result<Vec<Attribute>, String>> {
        Ok(self.repo().and_then(|repo| {
            check_attr(repo, &attributes, &path).map_err(|error| format!("{error:#}"))
        }))
    }

    fn cat_file(&mut self, object: String) -> wasmtime::Result<Result<Vec<u8>, String>> {
        Ok(self
            .repo()
            .and_then(|repo| cat_file(repo, &object).map_err(|error| format!("{error:#}"))))
    }
}

/// `git check-attr`: each attribute's state for `path`, with git's
/// precedence (info/attributes, .gitattributes files, user, system).
fn check_attr(
    repo: &gix::Repository,
    attributes: &[String],
    path: &str,
) -> anyhow::Result<Vec<Attribute>> {
    let index = repo.index_or_empty()?;
    let mut stack = repo.attributes_only(
        &index,
        gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
    )?;
    let mut matches = stack.selected_attribute_matches(attributes.iter().map(String::as_str));
    stack.at_path(path, None)?.matching_attributes(&mut matches);
    matches
        .iter_selected()
        .map(|matched| {
            Ok(match matched.assignment.state {
                AttrState::Unspecified => Attribute::Unspecified,
                AttrState::Unset => Attribute::Unset,
                AttrState::Set => Attribute::Set,
                AttrState::Value(value) => Attribute::Value(
                    value
                        .as_bstr()
                        .to_str()
                        .with_context(|| format!("{path}: an attribute value is not UTF-8"))?
                        .to_owned(),
                ),
            })
        })
        .collect()
}

/// `git cat-file blob <object>`.
fn cat_file(repo: &gix::Repository, object: &str) -> anyhow::Result<Vec<u8>> {
    let id = gix::ObjectId::from_hex(object.as_bytes())
        .map_err(|error| anyhow::anyhow!("{object:?} is not an object id: {error}"))?;
    Ok(repo.find_blob(id)?.detach().data)
}

/// One enabled shape plugin, compiled and linked once and instantiated by
/// every worker.
struct Component {
    name: Arc<str>,
    /// How configuration names it, such as `bundled.context`.
    reference: String,
    pre: DiffrPluginPre<State>,
    /// JSON for the plugin's constructor.
    options: String,
}

/// The classifier, compiled and linked once and instantiated by every worker.
struct Classifier {
    pre: DiffrClassifierPre<State>,
    /// JSON for the classifier's constructor.
    options: String,
}

/// Every plugin a worker instantiates.
struct Plugins {
    classifier: Classifier,
    shape: Vec<Component>,
}

/// Work for whichever worker is free first.
enum Job {
    /// The classifier's verdict on a file.
    Classify(FileEntry, oneshot::Sender<anyhow::Result<Classified>>),
    /// Walk the file with every shape plugin in order.
    Run(Cursor, oneshot::Sender<anyhow::Result<Cursor>>),
}

/// The classifier and the enabled shape plugins, run by a pool of workers.
/// Each worker holds an instance of every plugin; dropping the pipeline
/// closes the queue and the workers stop once their files are done.
pub(crate) struct Pipeline {
    jobs: async_channel::Sender<Job>,
}

impl Pipeline {
    /// Compile the classifier and every enabled shape plugin once, and start
    /// `workers` workers. Returns once every worker has made its instances,
    /// so a plugin that cannot be made fails here.
    pub(crate) fn from_config(
        config: &crate::config::Config,
        workdir: &Path,
        workers: NonZeroUsize,
    ) -> anyhow::Result<Self> {
        let engine = engine()?;
        let mut linker = Linker::<State>::new(&engine);
        wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
        wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)?;
        wasmtime_wasi::p3::add_to_linker(&mut linker)?;
        wasmtime_wasi_http::p3::add_to_linker(&mut linker)?;
        // The shape world's imports include everything the classifier's do.
        bindings::DiffrPlugin::add_to_linker::<State, HasSelf<State>>(&mut linker, |state| state)?;
        let classifier = Classifier {
            pre: DiffrClassifierPre::new(link(
                &engine,
                &linker,
                &config.classifier.folder().component(),
            )?)
            .map_err(anyhow::Error::from)
            .context("not a classifier: it must export diffr:plugin/classify")
            .context("classifier")?,
            options: Value::Object(config.classifier.options.clone()).to_string(),
        };
        let shape = config
            .plugins
            .enabled()
            .map(|(reference, entry)| {
                compile(&engine, &linker, reference, entry)
                    .with_context(|| format!("plugins.{reference}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let plugins = Arc::new(Plugins { classifier, shape });
        let (jobs, queue) = async_channel::unbounded();
        let started: Vec<_> = (0..workers.get())
            .map(|_| spawn_worker(&engine, plugins.clone(), workdir, queue.clone()))
            .collect::<anyhow::Result<_>>()?;
        for made in started {
            made.blocking_recv()
                .context("plugin worker stopped while starting")??;
        }
        Ok(Self { jobs })
    }

    /// The classifier's verdict on each file, in order.
    pub(crate) fn classify(
        &self,
        files: &[protocol::FileChange],
    ) -> anyhow::Result<Vec<Classified>> {
        let replies = files
            .iter()
            .map(|file| {
                let (reply, result) = oneshot::channel();
                self.jobs
                    .send_blocking(Job::Classify(file_entry(file), reply))
                    .map_err(|_| trapped())?;
                Ok(result)
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        replies
            .into_iter()
            .map(|result| result.blocking_recv().map_err(|_| trapped())?)
            .collect()
    }

    /// Run every shape plugin on one file's sides, on whichever worker is
    /// free. Returns the edited sides.
    pub(crate) async fn run(
        &self,
        file: &protocol::FileChange,
        sides: Pairing<protocol::Source>,
    ) -> anyhow::Result<Pairing<protocol::Source>> {
        let (reply, result) = oneshot::channel();
        self.jobs
            .send(Job::Run(Cursor::new(file_entry(file), sides), reply))
            .await
            .map_err(|_| trapped())?;
        let cursor = result.await.map_err(|_| trapped())??;
        Ok(cursor.sides)
    }
}

/// A worker dropped a file's reply, or every worker is gone: a plugin
/// trapped. The trap itself is logged to stderr by the worker.
fn trapped() -> anyhow::Error {
    anyhow::anyhow!("a plugin trapped; its error is on stderr")
        .context(MutationFailed("plugin worker".into()))
}

/// Compile a component and link it against the host's imports.
fn link(
    engine: &Engine,
    linker: &Linker<State>,
    source: &ComponentSource,
) -> anyhow::Result<wasmtime::component::InstancePre<State>> {
    let started = Instant::now();
    let (component, label) = match source {
        ComponentSource::File(path) => (
            Compiled::from_file(engine, path),
            path.display().to_string(),
        ),
        ComponentSource::Bundled(bytes) => {
            (Compiled::new(engine, bytes), "bundled component".into())
        }
    };
    let component = component
        .map_err(anyhow::Error::from)
        .with_context(|| format!("compiling {label}"))?;
    let pre = linker
        .instantiate_pre(&component)
        .map_err(anyhow::Error::from)
        .with_context(|| format!("linking {label}"))?;
    log::debug!("compiled and linked {label} in {:?}", started.elapsed());
    Ok(pre)
}

fn compile(
    engine: &Engine,
    linker: &Linker<State>,
    reference: &str,
    entry: &Entry,
) -> anyhow::Result<Component> {
    let pre = DiffrPluginPre::new(link(engine, linker, &entry.folder().component())?)
        .map_err(anyhow::Error::from)
        .context("not a shape plugin: it must export diffr:plugin/api")?;
    let name = reference
        .split_once('.')
        .map_or(reference, |(_, name)| name);
    Ok(Component {
        name: name.into(),
        reference: reference.to_owned(),
        pre,
        options: Value::Object(entry.options.clone()).to_string(),
    })
}

/// Start a worker thread and report once it has made every plugin, or why
/// it could not.
fn spawn_worker(
    engine: &Engine,
    plugins: Arc<Plugins>,
    workdir: &Path,
    jobs: async_channel::Receiver<Job>,
) -> anyhow::Result<oneshot::Receiver<anyhow::Result<()>>> {
    let (ready, made) = oneshot::channel();
    let engine = engine.clone();
    let workdir = workdir.to_owned();
    std::thread::Builder::new()
        .name("diffr-plugins".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = ready.send(Err(error.into()));
                    return;
                }
            };
            runtime.block_on(async {
                match Worker::make(&engine, &plugins, &workdir).await {
                    Ok(worker) => {
                        if ready.send(Ok(())).is_ok() {
                            worker.serve(jobs).await;
                        }
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                    }
                }
            });
        })?;
    Ok(made)
}

/// One shape plugin's instance in a worker and its constructed configuration.
struct Instance {
    name: Arc<str>,
    exports: DiffrPlugin,
    configured: ResourceAny,
}

/// One worker's Store, its classifier and its instance of every shape plugin.
struct Worker {
    store: Store<State>,
    classifier: (DiffrClassifier, ResourceAny),
    instances: Vec<Instance>,
}

impl Worker {
    /// Instantiate every component and run its constructor.
    async fn make(engine: &Engine, plugins: &Plugins, workdir: &Path) -> anyhow::Result<Self> {
        let started = Instant::now();
        let mut store = Store::new(engine, State::new(workdir)?);
        let classifier = plugins.classifier.pre.instantiate_async(&mut store).await?;
        let mut exports = Vec::new();
        for component in &plugins.shape {
            exports.push(component.pre.instantiate_async(&mut store).await?);
        }
        let (classifier, instances) = store
            .run_concurrent(async |accessor| -> anyhow::Result<_> {
                let configured = classifier
                    .diffr_plugin_classify()
                    .classifier()
                    .call_constructor(accessor, plugins.classifier.options.clone())
                    .await?
                    .map_err(anyhow::Error::msg)
                    .context("classifier")?;
                let mut instances = Vec::new();
                for (component, exports) in plugins.shape.iter().zip(exports) {
                    let configured = exports
                        .diffr_plugin_api()
                        .plugin()
                        .call_constructor(accessor, component.options.clone())
                        .await?
                        .map_err(anyhow::Error::msg)
                        .with_context(|| format!("plugins.{}", component.reference))?;
                    instances.push(Instance {
                        name: component.name.clone(),
                        exports,
                        configured,
                    });
                }
                Ok(((classifier, configured), instances))
            })
            .await??;
        log::debug!("plugin worker made in {:?}", started.elapsed());
        Ok(Self {
            store,
            classifier,
            instances,
        })
    }

    /// Serve jobs until the queue closes.
    async fn serve(self, jobs: async_channel::Receiver<Job>) {
        let Self {
            mut store,
            classifier,
            instances,
            ..
        } = self;
        let served = store
            .run_concurrent(async |accessor| {
                let classifier = &classifier;
                let instances = &instances;
                let mut pending: FuturesUnordered<futures::future::BoxFuture<'_, ()>> =
                    FuturesUnordered::new();
                let mut open = true;
                while open || !pending.is_empty() {
                    tokio::select! {
                        job = jobs.recv(), if open => {
                            let Ok(job) = job else {
                                open = false;
                                continue;
                            };
                            pending.push(Box::pin(async move {
                                match job {
                                    Job::Classify(file, reply) => {
                                        let _ = reply.send(classify(accessor, classifier, file).await);
                                    }
                                    Job::Run(cursor, reply) => {
                                        let _ = reply.send(run_chain(accessor, instances, cursor).await);
                                    }
                                }
                            }));
                            // Wasmtime runs queued guest calls only once this future
                            // yields. Yield before taking another job so a computing job
                            // does not block the queue.
                            let _ = futures::poll!(pending.next());
                            tokio::task::yield_now().await;
                        }
                        _ = pending.next(), if !pending.is_empty() => {}
                    }
                }
            })
            .await;
        if let Err(error) = served {
            log::error!("plugin worker trapped: {error:#}");
        }
        if let Err(error) = classifier.1.resource_drop_async(&mut store).await {
            log::error!("classifier: dropping configuration: {error:#}");
        }
        for instance in instances {
            if let Err(error) = instance.configured.resource_drop_async(&mut store).await {
                log::error!(
                    "plugin {}: dropping configuration: {error:#}",
                    instance.name
                );
            }
        }
    }
}

/// What the classifier decided about one file.
pub(crate) struct Classified {
    /// Tag names, sorted and deduplicated.
    pub(crate) tags: Vec<String>,
    /// Hide the file behind this reason.
    pub(crate) hidden: Option<String>,
}

/// The classifier's verdict on a file, its tags as names.
async fn classify(
    accessor: &Accessor<State>,
    (exports, configured): &(DiffrClassifier, ResourceAny),
    file: FileEntry,
) -> anyhow::Result<Classified> {
    let path = match &file.file {
        types::FileSides::Both((_, rhs)) | types::FileSides::RightOnly(rhs) => rhs.path.clone(),
        types::FileSides::LeftOnly(lhs) => lhs.path.clone(),
    };
    let classification = exports
        .diffr_plugin_classify()
        .classifier()
        .call_classify(accessor, *configured, file)
        .await?
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("classifier: {path}"))?;
    let names = classification
        .tags
        .into_iter()
        .map(|tag| match tag {
            Tag::Generated => Ok(crate::tags::GENERATED.to_owned()),
            Tag::Vendored => Ok("vendored".to_owned()),
            Tag::Docs => Ok("docs".to_owned()),
            Tag::Test => Ok("test".to_owned()),
            Tag::Custom(name) if crate::tags::is_tag(&name) => Ok(name),
            Tag::Custom(name) => Err(anyhow::anyhow!(
                "classifier: {path}: {name:?} is not a tag; use lowercase letters, digits, '-' and '_'"
            )),
        })
        .collect::<anyhow::Result<BTreeSet<String>>>()?;
    Ok(Classified {
        tags: names.into_iter().collect(),
        hidden: classification.hidden,
    })
}

/// Walk the file with every plugin in order.
async fn run_chain(
    accessor: &Accessor<State>,
    instances: &[Instance],
    cursor: Cursor,
) -> anyhow::Result<Cursor> {
    let handle = accessor.with(|mut access| access.data_mut().table.push(cursor))?;
    let mut walked = Ok(());
    for instance in instances {
        let started = Instant::now();
        walked = async {
            accessor.with(|mut access| -> anyhow::Result<()> {
                access.data_mut().table.get_mut(&handle)?.rewind();
                Ok(())
            })?;
            walk(
                accessor,
                &instance.exports.diffr_plugin_api().plugin(),
                instance.configured,
                &handle,
            )
            .await
        }
        .await
        .with_context(|| MutationFailed(instance.name.to_string()));
        log::debug!(
            "plugin {}: walk took {:?}",
            instance.name,
            started.elapsed()
        );
        if walked.is_err() {
            break;
        }
    }
    let cursor = accessor.with(|mut access| access.data_mut().table.delete(handle))?;
    walked?;
    Ok(cursor)
}
