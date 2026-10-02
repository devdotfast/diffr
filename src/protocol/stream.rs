//! The stdout stream: manifest, one record per file as it finishes, footer.
//!
//! Files are read from the repository serially and diffed on worker threads;
//! each becomes its record ([`super::record`]) as it finishes.
use super::project::{self, Inputs};
use super::record::{enrich_event, shape, syntax_spans, wire_error, Options};
use super::{Diff, Event, FileChange, Outcome, Snapshot, Source, Visibility, VERSION};
use crate::engine::QueryConflict;
use crate::git::{DiffSession, LoadedFile};
use crate::pairing::Pairing;
use crate::plugin::Pipeline;
use crate::summary::DiffResult;
use anyhow::anyhow;
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;

/// What the stream ended with: whether any file failed, and whether a
/// run-level failure cut it short.
pub struct Ended {
    pub failed: bool,
    pub aborted: bool,
}

/// Files are diffed on `jobs` workers and emitted as they finish. The queue
/// holds at most one ready record, so computation overlaps output without
/// retaining the whole comparison.
pub fn write(
    session: DiffSession,
    jobs: usize,
    pipeline: Arc<Pipeline>,
    options: Options,
    output: &mut impl Write,
) -> anyhow::Result<Ended> {
    let manifest = manifest(&session);
    let (sender, receiver) = sync_channel(1);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .thread_name(|index| format!("diffr-worker-{index}"))
        .build()?;
    let worker = thread::spawn(move || {
        // A disconnected consumer cancels production after the files in flight.
        let _ = produce(session, manifest, &pool, &pipeline, options, sender);
    });
    let mut output = BufWriter::new(output);
    let result: anyhow::Result<Ended> = (|| {
        let mut ended = Ended {
            failed: false,
            aborted: false,
        };
        for event in &receiver {
            if let Event::Complete {
                failed, aborted, ..
            } = &event
            {
                ended.failed |= *failed > 0;
                ended.aborted = aborted.is_some();
            }
            if matches!(&event, Event::Annotations { error: Some(_), .. }) {
                ended.failed = true;
            }
            serde_json::to_writer(&mut output, &event)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
        Ok(ended)
    })();
    // Wake a producer blocked on a full queue if writing failed.
    drop(receiver);
    let joined = worker.join();
    let ended = result?;
    joined.map_err(|_| anyhow!("diff computation thread panicked"))?;
    Ok(ended)
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
    pipeline: &Pipeline,
    options: Options,
    sender: SyncSender<Event>,
) -> Result<(), Disconnected> {
    let send = |event: Event| sender.send(event).map_err(|_| Disconnected);
    let start = Event::Start {
        version: if options.updates { 4 } else { VERSION },
        lhs: Snapshot::from(&session.comparison.before),
        rhs: Snapshot::from(&session.comparison.after),
        files: manifest,
    };
    send(start)?;
    let succeeded = AtomicU32::new(0);
    let failed = AtomicU32::new(0);
    let cancelled = Arc::new(AtomicBool::new(false));
    let aborted: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    let loader = Loader {
        session,
        cancelled: Arc::clone(&cancelled),
    };
    let (enrich_sender, enrich_receiver) =
        sync_channel::<(FileChange, Pairing<Source>)>(pool.current_num_threads());
    let enrich_receiver = Mutex::new(enrich_receiver);
    thread::scope(|scope| {
        let enrich_receiver = &enrich_receiver;
        if options.updates {
            for _ in 0..pool.current_num_threads() {
                let sender = &sender;
                let cancelled = &cancelled;
                scope.spawn(move || loop {
                    let job = enrich_receiver.lock().expect("enrichment queue").recv();
                    let Ok((entry, sides)) = job else {
                        break;
                    };
                    if cancelled.load(Ordering::Relaxed) {
                        continue;
                    }
                    let event = enrich_event(pipeline, &entry, &sides);
                    if sender.send(event).is_err() {
                        cancelled.store(true, Ordering::Relaxed);
                    }
                });
            }
        }
        pool.install(|| {
            loader.par_bridge().for_each(|(file, loaded)| {
                let (visibility, outcome) = match loaded.and_then(|loaded| diffed(&loaded, options))
                {
                    Ok((entry, diff)) => match shape(pipeline, &entry, diff, options.updates) {
                        Ok((visibility, diff)) => (visibility, Outcome::Diff { diff }),
                        Err(error) => {
                            // A run-level failure: stop pulling files, let the ones
                            // in flight finish, and report why in the footer.
                            failed.fetch_add(1, Ordering::Relaxed);
                            cancelled.store(true, Ordering::Relaxed);
                            aborted.lock().expect("abort reason").get_or_insert(error);
                            return;
                        }
                    },
                    Err(error) => (
                        Visibility::default(),
                        Outcome::Error {
                            error: wire_error(&error),
                        },
                    ),
                };
                match &outcome {
                    Outcome::Diff { .. } => succeeded.fetch_add(1, Ordering::Relaxed),
                    Outcome::Error { .. } => failed.fetch_add(1, Ordering::Relaxed),
                };
                let pending = if options.updates {
                    match &outcome {
                        Outcome::Diff {
                            diff: Diff::Text { sides, .. },
                        } => Some((file.manifest_entry(), sides.clone())),
                        _ => None,
                    }
                } else {
                    None
                };
                let event = Event::File {
                    file: file.sides,
                    visibility,
                    outcome,
                };
                if sender.send(event).is_err() {
                    cancelled.store(true, Ordering::Relaxed);
                } else if let Some(pending) = pending {
                    if !cancelled.load(Ordering::Relaxed) && enrich_sender.send(pending).is_err() {
                        cancelled.store(true, Ordering::Relaxed);
                    }
                }
            });
        });
        drop(enrich_sender);
    });
    let aborted = aborted.into_inner().expect("abort reason");
    send(Event::Complete {
        succeeded: succeeded.into_inner(),
        failed: failed.into_inner(),
        aborted: aborted.map(|error| wire_error(&error)),
    })
}

/// The projected diff of one loaded file and its manifest entry. `Err` is
/// this file's failure.
fn diffed(loaded: &LoadedFile, options: Options) -> anyhow::Result<(FileChange, Diff)> {
    let result = loaded.diff()?;
    let syntax = match options.syntax {
        true => syntax_spans(&result, &loaded.params),
        false => (Vec::new(), Vec::new()),
    };
    let inputs = Inputs {
        file: &loaded.file.sides,
        sizes: loaded.sizes(),
        syntax,
    };
    Ok((loaded.file.manifest_entry(), project::diff(&result, inputs)))
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
pub fn write_file(
    before: &str,
    after: &str,
    sizes: (u64, u64),
    compute: impl FnOnce() -> Result<DiffResult, QueryConflict>,
    params: &crate::params::Params,
    pipeline: &Pipeline,
    options: Options,
    output: &mut impl Write,
) -> anyhow::Result<Ended> {
    let file = crate::git::FileChange::standalone(before, after);
    let entry = file.manifest_entry();
    let mut output = BufWriter::new(output);
    let start = Event::Start {
        version: if options.updates { 4 } else { VERSION },
        lhs: Snapshot::Path {
            path: before.to_owned(),
        },
        rhs: Snapshot::Path {
            path: after.to_owned(),
        },
        files: vec![entry.clone()],
    };
    serde_json::to_writer(&mut output, &start)?;
    output.write_all(b"\n")?;
    output.flush()?;
    let (record, failed, aborted) = match compute() {
        Err(conflict) => (
            Some(Event::File {
                file: file.sides,
                visibility: Visibility::default(),
                outcome: Outcome::Error {
                    error: wire_error(&anyhow::Error::from(conflict)),
                },
            }),
            true,
            None,
        ),
        Ok(result) => {
            let projected = project::diff(
                &result,
                Inputs {
                    file: &file.sides,
                    sizes,
                    syntax: match options.syntax {
                        true => syntax_spans(&result, params),
                        false => (Vec::new(), Vec::new()),
                    },
                },
            );
            match shape(pipeline, &entry, projected, options.updates) {
                Ok((visibility, diff)) => (
                    Some(Event::File {
                        file: file.sides,
                        visibility,
                        outcome: Outcome::Diff { diff },
                    }),
                    false,
                    None,
                ),
                Err(error) => (None, true, Some(wire_error(&error))),
            }
        }
    };
    if let Some(record) = &record {
        serde_json::to_writer(&mut output, record)?;
        output.write_all(b"\n")?;
    }
    output.flush()?;
    let mut enrichment_failed = false;
    if options.updates {
        if let Some(Event::File {
            outcome: Outcome::Diff {
                diff: Diff::Text { sides, .. },
            },
            ..
        }) = &record
        {
            let event = enrich_event(pipeline, &entry, sides);
            enrichment_failed = matches!(&event, Event::Annotations { error: Some(_), .. });
            serde_json::to_writer(&mut output, &event)?;
            output.write_all(b"\n")?;
        }
    }
    let ended = Ended {
        failed: failed || enrichment_failed,
        aborted: aborted.is_some(),
    };
    serde_json::to_writer(
        &mut output,
        &Event::Complete {
            succeeded: u32::from(matches!(
                &record,
                Some(Event::File {
                    outcome: Outcome::Diff { .. },
                    ..
                })
            )),
            failed: u32::from(failed),
            aborted,
        },
    )?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(ended)
}

#[cfg(test)]
mod conflict_tests {
    use super::*;
    use crate::config::try_with_queries;

    #[test]
    fn a_query_conflict_is_that_files_error_and_the_run_completes() {
        let params = try_with_queries(&[(
            "rust",
            "((block) @fold (#set! tag \"removed-runs:whole\"))\n\
             ((block \"{\" @fold.open \"}\" @fold.close) @fold (#set! tag \"removed-runs:inside\"))\n",
        )])
        .unwrap();
        let mut output = Vec::new();
        let ended = write_file(
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
            &params,
            &Pipeline::default(),
            Options {
                syntax: false,
                updates: false,
            },
            &mut output,
        )
        .unwrap();
        assert!(ended.failed && !ended.aborted);
        let records: Vec<serde_json::Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let error = &records[1]["error"];
        assert_eq!(error["code"], "query_conflict", "{error}");
        let message = error["message"].as_str().unwrap();
        assert!(message.starts_with("src/lib.rs:1"), "{message}");
        assert!(
            message.contains("capture the same block with different fold ranges"),
            "{message}"
        );
        assert_eq!(records[2]["failed"], 1);
        assert!(records[2].get("aborted").is_none());
    }
}
