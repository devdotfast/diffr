//! The stdout stream: manifest, one record per file as it finishes, footer.
use super::project::{self, Inputs};
use super::{Event, FileChange, Outcome, Problem, Snapshot, VERSION};
use crate::git::{DiffSession, FileError, LoadedFile};
use crate::summary::DiffResult;
use anyhow::anyhow;
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::Arc;
use std::thread;

/// Returns whether any file failed. Files are diffed on `jobs` workers and
/// emitted as they finish. The queue holds at most one ready record, so
/// computation overlaps output without retaining the whole comparison.
pub(crate) fn write(
    session: DiffSession,
    jobs: usize,
    output: &mut impl Write,
) -> anyhow::Result<bool> {
    let manifest = manifest(&session);
    let (sender, receiver) = sync_channel(1);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .thread_name(|index| format!("diffr-worker-{index}"))
        .build()?;
    let worker = thread::spawn(move || {
        // A disconnected consumer cancels production after the files in flight.
        let _ = produce(session, manifest, &pool, sender);
    });
    let mut output = BufWriter::new(output);
    let result: anyhow::Result<bool> = (|| {
        let mut failed = false;
        for event in &receiver {
            if let Event::Complete { failed: count, .. } = &event {
                failed = *count > 0;
            }
            serde_json::to_writer(&mut output, &event)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
        Ok(failed)
    })();
    // Wake a producer blocked on a full queue if writing failed.
    drop(receiver);
    let joined = worker.join();
    let failed = result?;
    joined.map_err(|_| anyhow!("diff computation thread panicked"))?;
    Ok(failed)
}

/// The consumer went away; production stops after the files in flight.
struct Disconnected;

fn manifest(session: &DiffSession) -> Vec<FileChange> {
    session
        .file_manifest()
        .iter()
        .map(crate::git::FileChange::manifest_entry)
        .collect()
}

fn produce(
    session: DiffSession,
    manifest: Vec<FileChange>,
    pool: &rayon::ThreadPool,
    sender: SyncSender<Event>,
) -> Result<(), Disconnected> {
    let send = |event: Event| sender.send(event).map_err(|_| Disconnected);
    send(Event::Start {
        version: VERSION,
        lhs: Snapshot::from(&session.comparison.before),
        rhs: Snapshot::from(&session.comparison.after),
        files: manifest,
    })?;
    let succeeded = AtomicU32::new(0);
    let failed = AtomicU32::new(0);
    let cancelled = Arc::new(AtomicBool::new(false));
    let loader = Loader {
        session,
        cancelled: Arc::clone(&cancelled),
    };
    pool.install(|| {
        loader.par_bridge().for_each(|(file, loaded)| {
            let outcome = match loaded {
                Ok(loaded) => Outcome::Diff {
                    diff: file_diff(&loaded),
                },
                Err(error) => Outcome::Error {
                    error: wire_error(&error),
                },
            };
            match &outcome {
                Outcome::Diff { .. } => succeeded.fetch_add(1, Ordering::Relaxed),
                Outcome::Error { .. } => failed.fetch_add(1, Ordering::Relaxed),
            };
            let event = Event::File {
                file: file.sides,
                outcome,
            };
            if sender.send(event).is_err() {
                cancelled.store(true, Ordering::Relaxed);
            }
        });
    });
    send(Event::Complete {
        succeeded: succeeded.into_inner(),
        failed: failed.into_inner(),
        aborted: None,
    })
}

/// The wire record for an error, built as it is written. The code comes from
/// the typed cause attached where the error arose; an error nothing
/// classified is `internal`.
fn wire_error(error: &anyhow::Error) -> Problem {
    let code = match error.downcast_ref::<FileError>() {
        Some(kind) => kind.code(),
        None => "internal",
    };
    Problem {
        code: code.to_owned(),
        message: format!("{error:#}"),
    }
}

fn file_diff(loaded: &LoadedFile) -> super::Diff {
    let inputs = Inputs {
        file: &loaded.file.sides,
        sizes: loaded.sizes(),
        syntax: (Vec::new(), Vec::new()),
    };
    project::diff(&loaded.diff(), inputs)
}

/// Reads sources serially on whichever worker pulls next; diffing then
/// proceeds on that worker while others pull further files.
struct Loader {
    session: DiffSession,
    cancelled: Arc<AtomicBool>,
}

impl Iterator for Loader {
    type Item = (crate::git::FileChange, anyhow::Result<LoadedFile>);
    fn next(&mut self) -> Option<Self::Item> {
        if self.cancelled.load(Ordering::Relaxed) {
            return None;
        }
        self.session.load()
    }
}

/// A standalone two-path comparison through the same three records.
pub(crate) fn write_file(
    before: &str,
    after: &str,
    sizes: (u64, u64),
    compute: impl FnOnce() -> DiffResult,
    output: &mut impl Write,
) -> anyhow::Result<()> {
    let file = crate::git::FileChange::standalone(before, after);
    let mut output = BufWriter::new(output);
    serde_json::to_writer(
        &mut output,
        &Event::Start {
            version: VERSION,
            lhs: Snapshot::Path {
                path: before.to_owned(),
            },
            rhs: Snapshot::Path {
                path: after.to_owned(),
            },
            files: vec![file.manifest_entry()],
        },
    )?;
    output.write_all(b"\n")?;
    output.flush()?;
    let diff = project::diff(
        &compute(),
        Inputs {
            file: &file.sides,
            sizes,
            syntax: (Vec::new(), Vec::new()),
        },
    );
    serde_json::to_writer(
        &mut output,
        &Event::File {
            file: file.sides,
            outcome: Outcome::Diff { diff },
        },
    )?;
    output.write_all(b"\n")?;
    serde_json::to_writer(
        &mut output,
        &Event::Complete {
            succeeded: 1,
            failed: 0,
            aborted: None,
        },
    )?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}
