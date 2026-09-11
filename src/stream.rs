//! Incremental stdout protocol over the shared file iterator.
use crate::git::{DiffSession, FileChange, LoadedFile, Operand};
use crate::summary::DiffResult;
use rayon::iter::{ParallelBridge, ParallelIterator};
use serde::Serialize;
use serde_json::Value;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{sync_channel, SendError, SyncSender};
use std::sync::Arc;
use std::thread;

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    Start {
        version: u32,
        before: Operand,
        after: Operand,
        total: usize,
        files: Vec<FileChange>,
    },
    File {
        file: FileChange,
        diff: Value,
        /// The fold hook failed for this file; its folds keep their placeholders.
        #[serde(skip_serializing_if = "Option::is_none")]
        hook_error: Option<String>,
    },
    FileError {
        file: FileChange,
        message: String,
    },
    Complete {
        succeeded: usize,
        failed: usize,
    },
}

/// Returns whether any file failed. Files are diffed on `jobs` workers and
/// emitted as they finish, so results arrive in completion order. The queue
/// holds at most one ready event; computation can overlap output without
/// retaining the whole diff.
pub(crate) fn write(
    session: DiffSession,
    jobs: usize,
    output: &mut impl Write,
) -> crate::git::Result<bool> {
    let (sender, receiver) = sync_channel(1);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .thread_name(|index| format!("diffr-worker-{index}"))
        .build()?;
    let worker = thread::spawn(move || {
        // A disconnected consumer cancels production after the files in flight.
        let _ = produce(session, &pool, sender);
    });
    let mut output = BufWriter::new(output);
    let result: crate::git::Result<bool> = (|| {
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
    joined.map_err(|_| "diff computation thread panicked")?;
    Ok(failed)
}

fn produce(
    session: DiffSession,
    pool: &rayon::ThreadPool,
    sender: SyncSender<Event>,
) -> std::result::Result<(), SendError<Event>> {
    sender.send(Event::Start {
        version: 1,
        before: session.comparison.before.clone(),
        after: session.comparison.after.clone(),
        total: session.remaining(),
        files: session.file_manifest(),
    })?;
    let succeeded = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    let cancelled = Arc::new(AtomicBool::new(false));
    let loader = Loader {
        session,
        cancelled: Arc::clone(&cancelled),
    };
    pool.install(|| {
        loader.par_bridge().for_each(|(file, loaded)| {
            let event = match loaded {
                Ok(loaded) => {
                    succeeded.fetch_add(1, Ordering::Relaxed);
                    file_event(file, loaded.diff())
                }
                Err(error) => {
                    failed.fetch_add(1, Ordering::Relaxed);
                    Event::FileError {
                        file,
                        message: error.to_string(),
                    }
                }
            };
            if sender.send(event).is_err() {
                cancelled.store(true, Ordering::Relaxed);
            }
        });
    });
    sender.send(Event::Complete {
        succeeded: succeeded.into_inner(),
        failed: failed.into_inner(),
    })
}

/// The previous stream never carries summaries; hooks are a v2 feature.
fn file_event(file: FileChange, diff: DiffResult) -> Event {
    Event::File {
        file,
        diff: diff.domain_json(),
        hook_error: None,
    }
}

/// Reads sources serially on whichever worker pulls next; diffing then
/// proceeds on that worker while others pull further files.
struct Loader {
    session: DiffSession,
    cancelled: Arc<AtomicBool>,
}

impl Iterator for Loader {
    type Item = (FileChange, Result<LoadedFile, crate::git::FileProblem>);
    fn next(&mut self) -> Option<Self::Item> {
        if self.cancelled.load(Ordering::Relaxed) {
            return None;
        }
        self.session.load()
    }
}

/// Stream a standalone file comparison through the same file/completion events.
/// File operands identify paths rather than repository revisions.
pub(crate) fn write_file(
    before: &str,
    after: &str,
    compute: impl FnOnce() -> DiffResult,
    output: &mut impl Write,
) -> crate::git::Result<()> {
    let file = FileChange::standalone(before, after);
    let mut output = BufWriter::new(output);
    serde_json::to_writer(
        &mut output,
        &serde_json::json!({
            "type": "start", "version": 1, "total": 1, "files": [&file],
            "before": {"kind": "file", "path": before},
            "after": {"kind": "file", "path": after}
        }),
    )?;
    output.write_all(b"\n")?;
    output.flush()?;
    serde_json::to_writer(&mut output, &file_event(file, compute()))?;
    output.write_all(b"\n")?;
    serde_json::to_writer(
        &mut output,
        &Event::Complete {
            succeeded: 1,
            failed: 0,
        },
    )?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_manifest_is_flushed_before_computation() {
        struct Disconnected(Vec<u8>);
        impl Write for Disconnected {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Err(std::io::Error::from(std::io::ErrorKind::BrokenPipe))
            }
        }
        let mut output = Disconnected(vec![]);
        let result = write_file(
            "before.rs",
            "after.rs",
            || panic!("must not compute after manifest flush fails"),
            &mut output,
        );
        assert!(result.is_err());
        let start: Value = serde_json::from_slice(&output.0).unwrap();
        assert_eq!(start["type"], "start");
        assert_eq!(start["files"][0]["new_path"], "after.rs");
    }
}
