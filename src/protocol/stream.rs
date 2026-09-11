//! The v2 stdout stream: manifest, one record per file as it finishes, footer.
use super::project::{self, Inputs};
use super::{Event, FileChange, Outcome, Pairing, Problem, Snapshot, SyntaxSpan, VERSION};
use crate::git::{DiffSession, FileProblem, LoadedFile};
use crate::hook::Hook;
use crate::summary::{DiffResult, FileContent, FileFormat};
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;

/// Runtime choices that shape every file record.
#[derive(Clone, Copy)]
pub(crate) struct Options {
    /// Emit every token's capture name.
    pub(crate) syntax: bool,
}

/// What the stream ended with: whether any file failed, and whether the run
/// was cut short by a run-level failure.
pub(crate) struct Ended {
    pub(crate) failed: bool,
    pub(crate) aborted: bool,
}

/// Files are diffed on `jobs` workers and emitted as they finish. The queue
/// holds at most one ready record, so computation overlaps output without
/// retaining the whole comparison.
pub(crate) fn write(
    session: DiffSession,
    jobs: usize,
    hook: Option<Arc<Hook>>,
    options: Options,
    output: &mut impl Write,
) -> crate::git::Result<Ended> {
    let (sender, receiver) = sync_channel(1);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .thread_name(|index| format!("diffr-worker-{index}"))
        .build()?;
    let worker = thread::spawn(move || {
        // A disconnected consumer cancels production after the files in flight.
        let _ = produce(session, &pool, hook.as_deref(), options, sender);
    });
    let mut output = BufWriter::new(output);
    let result: crate::git::Result<Ended> = (|| {
        let mut ended = Ended {
            failed: false,
            aborted: false,
        };
        for event in &receiver {
            if let Event::Complete {
                failed, aborted, ..
            } = &event
            {
                ended.failed = *failed > 0;
                ended.aborted = aborted.is_some();
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
    joined.map_err(|_| "diff computation thread panicked")?;
    Ok(ended)
}

/// The consumer went away; production stops after the files in flight.
struct Disconnected;

fn produce(
    session: DiffSession,
    pool: &rayon::ThreadPool,
    hook: Option<&Hook>,
    options: Options,
    sender: SyncSender<Event>,
) -> Result<(), Disconnected> {
    let send = |event: Event| sender.send(event).map_err(|_| Disconnected);
    send(Event::Start {
        version: VERSION,
        lhs: Snapshot::from(&session.comparison.before),
        rhs: Snapshot::from(&session.comparison.after),
        files: session
            .file_manifest()
            .iter()
            .map(|file| file.manifest_entry())
            .collect(),
    })?;
    let succeeded = AtomicU32::new(0);
    let failed = AtomicU32::new(0);
    let cancelled = Arc::new(AtomicBool::new(false));
    let aborted: Mutex<Option<Problem>> = Mutex::new(None);
    let loader = Loader {
        session,
        cancelled: Arc::clone(&cancelled),
    };
    pool.install(|| {
        loader.par_bridge().for_each(|(file, loaded)| {
            let outcome = match loaded {
                Ok(loaded) => match file_outcome(&loaded, hook, options) {
                    Ok(outcome) => outcome,
                    Err(problem) => {
                        // A run-level failure: stop pulling files, let the ones in
                        // flight finish, and report why in the footer.
                        failed.fetch_add(1, Ordering::Relaxed);
                        cancelled.store(true, Ordering::Relaxed);
                        aborted.lock().expect("abort reason").get_or_insert(problem);
                        return;
                    }
                },
                Err(problem) => Outcome::Error {
                    error: problem_from(problem),
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
        aborted: aborted.into_inner().expect("abort reason"),
    })
}

fn problem_from(problem: FileProblem) -> Problem {
    Problem {
        code: problem.code.to_owned(),
        message: problem.message,
    }
}

/// `Err` here is a run-level failure, not a file-level one.
fn file_outcome(
    loaded: &LoadedFile,
    hook: Option<&Hook>,
    options: Options,
) -> Result<Outcome, Problem> {
    let mut diff = loaded.diff();
    if let Some(hook) = hook {
        hook.summarize(&mut diff).map_err(|message| Problem {
            code: "hook_failed".to_owned(),
            message,
        })?;
    }
    let syntax = if options.syntax {
        syntax_spans(&diff, &loaded.params)
    } else {
        (Vec::new(), Vec::new())
    };
    let inputs = Inputs {
        file: &loaded.file.sides,
        sizes: loaded.sizes(),
        context_lines: loaded.context_lines as usize,
        syntax,
    };
    Ok(project::diff(&diff, inputs).into())
}

pub(crate) fn syntax_spans(
    diff: &DiffResult,
    params: &crate::config::Params,
) -> (Vec<SyntaxSpan>, Vec<SyntaxSpan>) {
    let FileFormat::SupportedLanguage(language) = diff.file_format else {
        return (Vec::new(), Vec::new());
    };
    let parser = params.language(language).parser;
    let spans = |content: &FileContent| match content {
        FileContent::Text(src) => project::syntax_spans(src, parser),
        FileContent::Binary => Vec::new(),
    };
    (spans(&diff.lhs_src), spans(&diff.rhs_src))
}

/// Reads sources serially on whichever worker pulls next; diffing then
/// proceeds on that worker while others pull further files.
struct Loader {
    session: DiffSession,
    cancelled: Arc<AtomicBool>,
}

impl Iterator for Loader {
    type Item = (crate::git::FileChange, Result<LoadedFile, FileProblem>);
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
    params: &crate::config::Params,
    context_lines: usize,
    hook: Option<&Hook>,
    options: Options,
    output: &mut impl Write,
) -> crate::git::Result<Ended> {
    let file = crate::git::FileChange::standalone(before, after);
    let mut output = BufWriter::new(output);
    let manifest: FileChange = file.manifest_entry();
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
            files: vec![manifest],
        },
    )?;
    output.write_all(b"\n")?;
    output.flush()?;
    let mut diff = compute();
    let mut aborted = None;
    let outcome = match hook.map(|hook| hook.summarize(&mut diff)) {
        Some(Err(message)) => {
            aborted = Some(Problem {
                code: "hook_failed".to_owned(),
                message,
            });
            None
        }
        _ => {
            let syntax = if options.syntax {
                syntax_spans(&diff, params)
            } else {
                (Vec::new(), Vec::new())
            };
            let sides: Pairing<_> = file.sides.clone();
            Some(Outcome::from(project::diff(
                &diff,
                Inputs {
                    file: &sides,
                    sizes,
                    context_lines,
                    syntax,
                },
            )))
        }
    };
    let mut ended = Ended {
        failed: false,
        aborted: aborted.is_some(),
    };
    let (succeeded, failed) = match outcome {
        Some(outcome) => {
            ended.failed = matches!(outcome, Outcome::Error { .. });
            let counts = if ended.failed { (0, 1) } else { (1, 0) };
            serde_json::to_writer(
                &mut output,
                &Event::File {
                    file: file.sides,
                    outcome,
                },
            )?;
            output.write_all(b"\n")?;
            counts
        }
        None => (0, 1),
    };
    serde_json::to_writer(
        &mut output,
        &Event::Complete {
            succeeded,
            failed,
            aborted,
        },
    )?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(ended)
}
