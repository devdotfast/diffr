//! The stdout stream: manifest, one record per file as it finishes, footer.
use super::project::{self, Inputs};
use super::{
    Diff, Event, FileChange, LineCounts, Node, Outcome, Pairing, Problem, Region, Snapshot, Source,
    VERSION,
};
use crate::git::{DiffSession, FileError, LoadedFile};
use crate::hash::DftHashSet;
use crate::mutate::{Failure, Mutations};
use crate::summary::DiffResult;
use anyhow::anyhow;
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;

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
    output: &mut impl Write,
) -> anyhow::Result<Ended> {
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
        let _ = produce(session, manifest, &pool, &mutations, sender);
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
    joined.map_err(|_| anyhow!("diff computation thread panicked"))?;
    Ok(ended)
}

/// The consumer went away; production stops after the files in flight.
struct Disconnected;

fn manifest(session: &DiffSession, mutations: &Mutations) -> anyhow::Result<Vec<FileChange>> {
    session
        .file_manifest()
        .iter()
        .map(|file| {
            let mut entry = file.manifest_entry();
            mutations.apply_file(&mut entry)?;
            Ok(entry)
        })
        .collect()
}

fn produce(
    session: DiffSession,
    manifest: Vec<FileChange>,
    pool: &rayon::ThreadPool,
    mutations: &Mutations,
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
    let aborted: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    let loader = Loader {
        session,
        cancelled: Arc::clone(&cancelled),
    };
    pool.install(|| {
        loader.par_bridge().for_each(|(file, loaded)| {
            let outcome = match loaded {
                Ok(loaded) => match file_diff(&loaded, mutations) {
                    Ok(diff) => Outcome::Diff { diff },
                    Err(error) => {
                        // A run-level failure: stop pulling files, let the ones in
                        // flight finish, and report why in the footer.
                        failed.fetch_add(1, Ordering::Relaxed);
                        cancelled.store(true, Ordering::Relaxed);
                        aborted.lock().expect("abort reason").get_or_insert(error);
                        return;
                    }
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
        aborted: aborted
            .into_inner()
            .expect("abort reason")
            .map(|error| wire_error(&error)),
    })
}

/// The wire record for an error, built as it is written. The code comes from
/// the typed cause attached where the error arose; an error nothing
/// classified is `internal`.
fn wire_error(error: &anyhow::Error) -> Problem {
    let code = if let Some(kind) = error.downcast_ref::<FileError>() {
        kind.code()
    } else if let Some(failure) = error.downcast_ref::<Failure>() {
        failure.code()
    } else {
        "internal"
    };
    Problem {
        code: code.to_owned(),
        message: format!("{error:#}"),
    }
}

/// `Err` here is a run-level failure, not a file-level one.
fn file_diff(loaded: &LoadedFile, mutations: &Mutations) -> anyhow::Result<Diff> {
    let diff = loaded.diff();
    let inputs = Inputs {
        file: &loaded.file.sides,
        sizes: loaded.sizes(),
        syntax: (Vec::new(), Vec::new()),
    };
    let mut entry = loaded.file.manifest_entry();
    mutations.apply_file(&mut entry)?;
    mutate(mutations, &entry, project::diff(&diff, inputs))
}

/// Fold mutations rewrite a text diff's regions, and its visible counts are
/// recounted after them.
fn mutate(mutations: &Mutations, entry: &FileChange, diff: Diff) -> anyhow::Result<Diff> {
    match diff {
        Diff::Text {
            mut sides,
            mut stats,
        } => {
            mutations.apply_fold(entry, &mut sides)?;
            stats.visible = visible_counts(&sides);
            Ok(Diff::Text { sides, stats })
        }
        binary @ Diff::Binary { .. } => Ok(binary),
    }
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
    mutations: &Mutations,
    output: &mut impl Write,
) -> anyhow::Result<Ended> {
    let file = crate::git::FileChange::standalone(before, after);
    let mut output = BufWriter::new(output);
    let mut entry: FileChange = file.manifest_entry();
    mutations.apply_file(&mut entry)?;
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
    let projected = project::diff(
        &compute(),
        Inputs {
            file: &file.sides,
            sizes,
            syntax: (Vec::new(), Vec::new()),
        },
    );
    let aborted = match mutate(mutations, &entry, projected) {
        Ok(diff) => {
            serde_json::to_writer(
                &mut output,
                &Event::File {
                    file: file.sides,
                    outcome: Outcome::Diff { diff },
                },
            )?;
            output.write_all(b"\n")?;
            None
        }
        Err(error) => Some(wire_error(&error)),
    };
    let ended = Ended {
        failed: false,
        aborted: aborted.is_some(),
    };
    serde_json::to_writer(
        &mut output,
        &Event::Complete {
            succeeded: u32::from(!ended.aborted),
            failed: u32::from(ended.aborted),
            aborted,
        },
    )?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(ended)
}

/// Changed lines that start on screen. A line counts when it carries a
/// `changed` span, or when its leaf exists on one side only (every line of
/// a one-sided leaf is new or removed, blank ones included). Lines inside a
/// collapsed region, or under one, are not counted.
pub(crate) fn visible_counts(sides: &Pairing<Source>) -> LineCounts {
    fn ids(regions: &[Region], out: &mut DftHashSet<u32>) {
        for region in regions {
            out.insert(region.alignment_id);
            if let Node::Fold { children } = &region.node {
                ids(children, out);
            }
        }
    }
    fn count(regions: &[Region], other: &DftHashSet<u32>, hidden: bool) -> u32 {
        let mut total = 0;
        for region in regions {
            let hidden = hidden || region.visibility.collapsed;
            match &region.node {
                Node::Leaf { changed } => {
                    if hidden {
                        continue;
                    }
                    if other.contains(&region.alignment_id) {
                        let lines: DftHashSet<u32> = changed.iter().map(|span| span.line).collect();
                        total += lines.len() as u32;
                    } else {
                        total += region.range.lines().len() as u32;
                    }
                }
                Node::Fold { children } => total += count(children, other, hidden),
            }
        }
        total
    }
    let side_ids = |source: &Source| {
        let mut out = DftHashSet::default();
        ids(&source.regions, &mut out);
        out
    };
    match sides {
        Pairing::Both { lhs, rhs } => LineCounts {
            added: count(&rhs.regions, &side_ids(lhs), false),
            removed: count(&lhs.regions, &side_ids(rhs), false),
        },
        Pairing::LeftOnly { lhs } => LineCounts {
            added: 0,
            removed: count(&lhs.regions, &DftHashSet::default(), false),
        },
        Pairing::RightOnly { rhs } => LineCounts {
            added: count(&rhs.regions, &DftHashSet::default(), false),
            removed: 0,
        },
    }
}

#[cfg(test)]
mod visible_tests {
    use super::*;
    use crate::protocol::{SourcePos, SourceRange, Span, Visibility};

    fn pos(line: u32) -> SourcePos {
        SourcePos { line, column: 0 }
    }

    fn leaf(id: u32, lines: (u32, u32), changed: &[u32], collapsed: bool) -> Region {
        Region {
            alignment_id: id,
            fold_state_id: id,
            range: SourceRange {
                start: pos(lines.0),
                end: pos(lines.1),
            },
            tags: vec![],
            visibility: Visibility {
                collapsed,
                label: String::new(),
            },
            node: Node::Leaf {
                changed: changed
                    .iter()
                    .map(|&line| Span {
                        line,
                        start_column: 0,
                        end_column: 1,
                    })
                    .collect(),
            },
        }
    }

    fn fold(id: u32, lines: (u32, u32), collapsed: bool, children: Vec<Region>) -> Region {
        Region {
            alignment_id: id,
            fold_state_id: id,
            range: SourceRange {
                start: pos(lines.0),
                end: pos(lines.1),
            },
            tags: vec!["body".to_owned()],
            visibility: Visibility {
                collapsed,
                label: String::new(),
            },
            node: Node::Fold { children },
        }
    }

    fn source(regions: Vec<Region>) -> Source {
        Source {
            text: String::new(),
            syntax: vec![],
            regions,
        }
    }

    #[test]
    fn counts_span_lines_and_every_line_of_a_one_sided_leaf() {
        let rhs = source(vec![
            // paired leaf: only the lines with spans count (two, one twice)
            leaf(1, (0, 3), &[0, 1, 1], false),
            // paired leaf without spans: unchanged context, not counted
            leaf(9, (3, 4), &[], false),
            // one-sided leaf with no spans (blank lines): every line counts
            leaf(2, (4, 6), &[], false),
            // collapsed leaf: hidden
            leaf(3, (6, 9), &[6, 7], true),
            // open fold with an open one-sided leaf: every line counts
            fold(4, (9, 12), false, vec![leaf(5, (9, 12), &[10], false)]),
            // collapsed fold: its open child is hidden by the ancestor
            fold(6, (12, 15), true, vec![leaf(7, (12, 15), &[13, 14], false)]),
        ]);
        let lhs = source(vec![
            leaf(1, (0, 3), &[0], false),
            leaf(9, (3, 4), &[], false),
            leaf(8, (4, 7), &[4, 5], true),
        ]);
        let counts = visible_counts(&Pairing::Both { lhs, rhs });
        assert_eq!(counts.added, 2 + 2 + 3);
        assert_eq!(counts.removed, 1);
    }

    #[test]
    fn a_missing_side_counts_nothing() {
        let rhs = source(vec![leaf(1, (0, 1), &[0], false)]);
        let counts = visible_counts(&Pairing::RightOnly { rhs });
        assert_eq!(counts.added, 1);
        assert_eq!(counts.removed, 0);
    }
}
