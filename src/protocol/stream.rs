//! The stdout stream: manifest, one record per file as it finishes, footer.
use super::project::{self, Inputs};
use super::{Event, FileChange, Outcome, Problem, Snapshot, VERSION};
use crate::engine::QueryConflict;
use crate::git::{DiffSession, FileError, LoadedFile};
use crate::options::DisplayOptions;
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
            let outcome = match loaded.and_then(|loaded| file_diff(&loaded)) {
                Ok(diff) => Outcome::Diff { diff },
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
    })
}

/// The wire record for an error, built as it is written. The code comes from
/// the typed cause attached where the error arose; an error nothing
/// classified is `internal`.
fn wire_error(error: &anyhow::Error) -> Problem {
    let code = if let Some(kind) = error.downcast_ref::<FileError>() {
        kind.code()
    } else if error.downcast_ref::<QueryConflict>().is_some() {
        "query_conflict"
    } else {
        "internal"
    };
    Problem {
        code: code.to_owned(),
        message: format!("{error:#}"),
    }
}

/// The projected diff of one loaded file. `Err` is this file's failure.
fn file_diff(loaded: &LoadedFile) -> anyhow::Result<super::Diff> {
    let inputs = Inputs {
        file: &loaded.file.sides,
        sizes: loaded.sizes(),
    };
    // The stream reads no terminal hunks, so their context is moot.
    let result = loaded.diff(&DisplayOptions::default())?;
    Ok(project::diff(&result, inputs))
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
    compute: impl FnOnce() -> Result<DiffResult, QueryConflict>,
    output: &mut impl Write,
) -> anyhow::Result<bool> {
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
    let outcome = match compute() {
        Ok(result) => Outcome::Diff {
            diff: project::diff(
                &result,
                Inputs {
                    file: &file.sides,
                    sizes,
                },
            ),
        },
        Err(conflict) => Outcome::Error {
            error: wire_error(&anyhow::Error::from(conflict)),
        },
    };
    let failed = matches!(outcome, Outcome::Error { .. });
    serde_json::to_writer(
        &mut output,
        &Event::File {
            file: file.sides,
            outcome,
        },
    )?;
    output.write_all(b"\n")?;
    serde_json::to_writer(
        &mut output,
        &Event::Complete {
            succeeded: u32::from(!failed),
            failed: u32::from(failed),
        },
    )?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(failed)
}

#[cfg(test)]
mod conflict_tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn a_query_conflict_is_that_files_error_and_the_run_completes() {
        let params = Config::from_toml(
            "[languages.rust]\nfolds = '''\n\
             ((block) @fold (#set! tag \"whole\"))\n\
             ((block \"{\" @fold.open \"}\" @fold.close) @fold (#set! tag \"inside\"))\n\
             '''",
        )
        .unwrap()
        .compile()
        .unwrap();
        let mut output = Vec::new();
        let failed = write_file(
            "a.rs",
            "b.rs",
            (0, 0),
            || {
                DiffResult::try_from_sources_with_params(
                    "src/lib.rs",
                    "fn f() {\n    one();\n}\n",
                    "fn f() {\n    two();\n}\n",
                    &params,
                )
            },
            &mut output,
        )
        .unwrap();
        assert!(failed);
        let records: Vec<serde_json::Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let error = &records[1]["error"];
        assert_eq!(error["code"], "query_conflict", "{error}");
        let message = error["message"].as_str().unwrap();
        assert_eq!(
            message,
            "src/lib.rs:1 (before): fold query patterns 0 and 1 capture the same block \
             with different fold ranges"
        );
        assert_eq!(records[2]["succeeded"], 0);
        assert_eq!(records[2]["failed"], 1);
    }
}
