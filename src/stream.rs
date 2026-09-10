//! Incremental stdout protocol over the shared file iterator.
use crate::git::{DiffSession, FileChange, Operand, Result};
use serde::Serialize;
use serde_json::Value;
use std::io::{BufWriter, Write};
use std::sync::mpsc::{sync_channel, SendError, SyncSender};
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

/// Returns whether any file failed. The queue holds at most one ready event;
/// computation can overlap output without retaining the whole diff.
pub(crate) fn write(session: DiffSession, output: &mut impl Write) -> Result<bool> {
    let (sender, receiver) = sync_channel(1);
    let worker = thread::spawn(move || {
        // A disconnected consumer cancels production after the current file.
        let _ = produce(session, sender);
    });
    let mut output = BufWriter::new(output);
    let result: Result<bool> = (|| {
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
    sender: SyncSender<Event>,
) -> std::result::Result<(), SendError<Event>> {
    sender.send(Event::Start {
        version: 1,
        before: session.comparison.before.clone(),
        after: session.comparison.after.clone(),
        total: session.remaining(),
        files: session.file_manifest(),
    })?;
    let mut succeeded = 0;
    let mut failed = 0;
    for (file, result) in session {
        let event = match result {
            Ok(diff) => {
                succeeded += 1;
                Event::File {
                    file,
                    diff: diff.domain_json(),
                }
            }
            Err(error) => {
                failed += 1;
                Event::FileError {
                    file,
                    message: error.to_string(),
                }
            }
        };
        sender.send(event)?;
    }
    sender.send(Event::Complete { succeeded, failed })
}

/// Stream a standalone file comparison through the same file/completion events.
/// File operands identify paths rather than repository revisions.
pub(crate) fn write_file(
    before: &str,
    after: &str,
    compute: impl FnOnce() -> crate::summary::DiffResult,
    output: &mut impl Write,
) -> Result<()> {
    let file = FileChange {
        old_path: (before != "/dev/null").then(|| before.into()),
        new_path: (after != "/dev/null").then(|| after.into()),
        status: if before == "/dev/null" {
            crate::git::FileStatus::Added
        } else if after == "/dev/null" {
            crate::git::FileStatus::Deleted
        } else {
            crate::git::FileStatus::Modified
        },
        class: None,
    };
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
    let diff = compute();
    serde_json::to_writer(
        &mut output,
        &Event::File {
            file,
            diff: diff.domain_json(),
        },
    )?;
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
