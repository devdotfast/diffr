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
