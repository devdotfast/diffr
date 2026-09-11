//! The v2 stdout stream: manifest, one record per file as it finishes, footer.
use super::project::{self, Inputs};
use super::{Diff, Event, FileChange, Outcome, Pairing, Problem, Snapshot, SyntaxSpan, VERSION};
use crate::git::{DiffSession, FileProblem, LoadedFile};
use crate::mutate::Mutations;
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
    mutations: Arc<Mutations>,
    options: Options,
    output: &mut impl Write,
) -> crate::git::Result<Ended> {
    // File mutations run before the manifest is written; a failure there is
    // a setup error, not a stream event.
    let manifest = manifest(&session, &mutations)?;
    let (sender, receiver) = sync_channel(1);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .thread_name(|index| format!("diffr-worker-{index}"))
        .build()?;
    let worker = thread::spawn(move || {
        // A disconnected consumer cancels production after the files in flight.
        let _ = produce(session, manifest, &pool, &mutations, options, sender);
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

fn manifest(session: &DiffSession, mutations: &Mutations) -> crate::git::Result<Vec<FileChange>> {
    session
        .file_manifest()
        .iter()
        .map(|file| {
            let mut entry = file.manifest_entry();
            mutations
                .apply_file(&mut entry)
                .map_err(|problem| format!("{}: {}", problem.code, problem.message))?;
            Ok(entry)
        })
        .collect()
}

fn produce(
    session: DiffSession,
    manifest: Vec<FileChange>,
    pool: &rayon::ThreadPool,
    mutations: &Mutations,
    options: Options,
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
    let aborted: Mutex<Option<Problem>> = Mutex::new(None);
    let loader = Loader {
        session,
        cancelled: Arc::clone(&cancelled),
    };
    pool.install(|| {
        loader.par_bridge().for_each(|(file, loaded)| {
            let outcome = match loaded {
                Ok(loaded) => match file_outcome(&loaded, mutations, options) {
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
    mutations: &Mutations,
    options: Options,
) -> Result<Outcome, Problem> {
    let diff = loaded.diff();
    let syntax = if options.syntax {
        let path = loaded.file.sides.rhs().or(loaded.file.sides.lhs());
        let path = path.map(|side| side.path.as_str()).unwrap_or_default();
        syntax_spans(&diff, &loaded.params, std::path::Path::new(path))
    } else {
        (Vec::new(), Vec::new())
    };
    let inputs = Inputs {
        file: &loaded.file.sides,
        sizes: loaded.sizes(),
        syntax,
    };
    let mut entry = loaded.file.manifest_entry();
    mutations.apply_file(&mut entry)?;
    mutate(mutations, &entry, project::diff(&diff, inputs))
}

/// Fold mutations see the projected text diff; a per-file failure passes
/// through untouched.
fn mutate(
    mutations: &Mutations,
    entry: &FileChange,
    projected: Result<Diff, Problem>,
) -> Result<Outcome, Problem> {
    let outcome = match projected {
        Ok(Diff::Text { mut sides, stats }) => {
            mutations.apply_fold(entry, &mut sides)?;
            Ok(Diff::Text { sides, stats })
        }
        other => other,
    };
    Ok(outcome.into())
}

/// Highlight spans for both sides. A line-diff fallback still has a
/// language, guessed from the path, so its sides get colours too.
pub(crate) fn syntax_spans(
    diff: &DiffResult,
    params: &crate::config::Params,
    path: &std::path::Path,
) -> (Vec<SyntaxSpan>, Vec<SyntaxSpan>) {
    let language = match &diff.file_format {
        FileFormat::SupportedLanguage(language) => Some(*language),
        FileFormat::TextFallback { .. } => {
            let sample = match (&diff.lhs_src, &diff.rhs_src) {
                (FileContent::Text(src), _) | (_, FileContent::Text(src)) => src.as_str(),
                _ => "",
            };
            crate::parse::guess_language::guess(path, sample, &[])
        }
        FileFormat::PlainText | FileFormat::Binary => None,
    };
    let Some(language) = language else {
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
    mutations: &Mutations,
    options: Options,
    output: &mut impl Write,
) -> crate::git::Result<Ended> {
    let file = crate::git::FileChange::standalone(before, after);
    let mut output = BufWriter::new(output);
    let mut entry: FileChange = file.manifest_entry();
    mutations
        .apply_file(&mut entry)
        .map_err(|problem| format!("{}: {}", problem.code, problem.message))?;
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
            files: vec![entry.clone()],
        },
    )?;
    output.write_all(b"\n")?;
    output.flush()?;
    let diff = compute();
    let syntax = if options.syntax {
        syntax_spans(&diff, params, std::path::Path::new(after))
    } else {
        (Vec::new(), Vec::new())
    };
    let sides: Pairing<_> = file.sides.clone();
    let projected = project::diff(
        &diff,
        Inputs {
            file: &sides,
            sizes,
            syntax,
        },
    );
    let mut aborted = None;
    let outcome = match mutate(mutations, &entry, projected) {
        Ok(outcome) => Some(outcome),
        Err(problem) => {
            aborted = Some(problem);
            None
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_files_still_get_syntax_spans() {
        let params = crate::config::Config::from_toml("")
            .unwrap()
            .compile()
            .unwrap();
        let src = "fn f() {\n    let x = 1;\n}\n";
        let diff = DiffResult::from_sources_with_options(
            "a.rs",
            "fn g() {\n    let y = 2;\n}\n",
            src,
            &params,
            &crate::options::DisplayOptions::default(),
            &crate::options::DiffOptions {
                graph_limit: 1,
                ..crate::options::DiffOptions::default()
            },
        );
        assert!(matches!(diff.file_format, FileFormat::TextFallback { .. }));
        let (lhs, rhs) = syntax_spans(&diff, &params, std::path::Path::new("a.rs"));
        assert!(!lhs.is_empty() && !rhs.is_empty());
        assert!(rhs.iter().any(|span| span.capture.starts_with("keyword")));
    }
}
