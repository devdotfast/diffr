//! The stdout stream: manifest, one record per file as it finishes, footer.
//!
//! Each file is diffed, projected, and shaped by the plugins
//! before its record is written; `stats.visible` is recounted after them.
use super::project::{self, Inputs};
use super::{
    Diff, Event, FileChange, LineCounts, Node, Outcome, Problem, Region, Snapshot, Source,
    SyntaxSpan, Visibility, VERSION,
};
use crate::engine::QueryConflict;
use crate::git::{DiffSession, FileError, LoadedFile};
use crate::hash::DftHashSet;
use crate::pairing::Pairing;
use crate::plugin::{MutationFailed, Pipeline};
use crate::summary::{DiffResult, FileContent, FileFormat};
use anyhow::anyhow;
use rayon::iter::{ParallelBridge, ParallelIterator};
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;

/// Runtime choices that shape every file record.
#[derive(Clone, Copy)]
pub(crate) struct Options {
    /// Emit every token's capture name (`--syntax`).
    pub(crate) syntax: bool,
}

/// What the stream ended with: whether any file failed, and whether a
/// run-level failure cut it short.
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
        version: VERSION,
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
    pool.install(|| {
        loader.par_bridge().for_each(|(file, loaded)| {
            let (visibility, outcome) = match loaded.and_then(|loaded| diffed(&loaded, options)) {
                Ok((entry, diff)) => match shape(pipeline, &entry, diff) {
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
            let event = Event::File {
                file: file.sides,
                visibility,
                outcome,
            };
            if sender.send(event).is_err() {
                cancelled.store(true, Ordering::Relaxed);
            }
        });
    });
    let aborted = aborted.into_inner().expect("abort reason");
    send(Event::Complete {
        succeeded: succeeded.into_inner(),
        failed: failed.into_inner(),
        aborted: aborted.map(|error| wire_error(&error)),
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
    } else if error.downcast_ref::<MutationFailed>().is_some() {
        "mutation_failed"
    } else {
        "internal"
    };
    Problem {
        code: code.to_owned(),
        message: format!("{error:#}"),
    }
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

/// Highlight spans for both sides of a structurally parsed file. A file that
/// fell back to a line diff has none.
fn syntax_spans(
    diff: &DiffResult,
    params: &crate::config::Params,
) -> (Vec<SyntaxSpan>, Vec<SyntaxSpan>) {
    let FileFormat::SupportedLanguage(language) = &diff.file_format else {
        return (Vec::new(), Vec::new());
    };
    let parser = params.language(*language).parser;
    let spans = |content: &FileContent| match content {
        FileContent::Text(src) => project::syntax_spans(src, parser),
        FileContent::Binary => Vec::new(),
    };
    (spans(&diff.lhs_src), spans(&diff.rhs_src))
}

/// Run the plugins on a diff and recount what stays visible. A binary diff has
/// no text and no regions, so only moves on the file apply to it. `Err`
/// is a run-level failure.
fn shape(
    pipeline: &Pipeline,
    entry: &FileChange,
    diff: Diff,
) -> anyhow::Result<(Visibility, Diff)> {
    match diff {
        Diff::Text {
            mut sides,
            mut stats,
        } => {
            let visibility = pipeline.run_diff(entry, &mut sides)?;
            stats.visible = visible_counts(&sides);
            Ok((visibility, Diff::Text { sides, stats }))
        }
        Diff::Binary { sides } => {
            let mut empty = sides.clone().map(|_| Source {
                text: String::new(),
                syntax: Vec::new(),
                regions: Vec::new(),
            });
            let visibility = pipeline.run_diff(entry, &mut empty)?;
            Ok((visibility, Diff::Binary { sides }))
        }
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
    compute: impl FnOnce() -> Result<DiffResult, QueryConflict>,
    params: &crate::config::Params,
    pipeline: &Pipeline,
    options: Options,
    output: &mut impl Write,
) -> anyhow::Result<Ended> {
    let file = crate::git::FileChange::standalone(before, after);
    let entry = file.manifest_entry();
    let mut output = BufWriter::new(output);
    let start = Event::Start {
        version: VERSION,
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
            match shape(pipeline, &entry, projected) {
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
    let ended = Ended {
        failed,
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

/// Changed lines that start on screen. A line counts when it carries a
/// `changed` span, or when no leaf on the other side shares its
/// `alignment_id` (every line of
/// a one-sided leaf is new or removed, blank ones included). Lines inside a
/// collapsed region, or under one, are not counted.
pub(crate) fn visible_counts(sides: &Pairing<Source>) -> LineCounts {
    fn alignments(regions: &[Region], out: &mut DftHashSet<u32>) {
        for region in regions {
            match &region.node {
                Node::Leaf { alignment_id, .. } => {
                    out.insert(*alignment_id);
                }
                Node::Fold { children } => alignments(children, out),
            }
        }
    }
    fn count(regions: &[Region], other: &DftHashSet<u32>, hidden: bool) -> u32 {
        let mut total = 0;
        for region in regions {
            let hidden = hidden || region.visibility.collapsed;
            match &region.node {
                Node::Leaf {
                    alignment_id,
                    changed,
                    ..
                } => {
                    if hidden {
                        continue;
                    }
                    if other.contains(alignment_id) {
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
    let side_alignments = |source: &Source| {
        let mut out = DftHashSet::default();
        alignments(&source.regions, &mut out);
        out
    };
    match sides {
        Pairing::Both { lhs, rhs } => LineCounts {
            added: count(&rhs.regions, &side_alignments(lhs), false),
            removed: count(&lhs.regions, &side_alignments(rhs), false),
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
    use crate::protocol::{SourcePos, SourceRange, Span};

    fn pos(line: u32) -> SourcePos {
        SourcePos { line, column: 0 }
    }

    fn leaf(
        id: u32,
        alignment: u32,
        lines: (u32, u32),
        changed: &[u32],
        collapsed: bool,
    ) -> Region {
        Region {
            id,
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
                alignment_id: alignment,
                search_highlights: Vec::new(),
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
            id,
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
            leaf(0, 1, (0, 3), &[0, 1, 1], false),
            // paired leaf without spans: unchanged context, not counted
            leaf(1, 9, (3, 4), &[], false),
            // one-sided leaf with no spans (blank lines): every line counts
            leaf(2, 2, (4, 6), &[], false),
            // collapsed leaf: hidden
            leaf(3, 3, (6, 9), &[6, 7], true),
            // open fold with an open one-sided leaf: every line counts
            fold(4, (9, 12), false, vec![leaf(5, 5, (9, 12), &[10], false)]),
            // collapsed fold: its open child is hidden by the ancestor
            fold(
                6,
                (12, 15),
                true,
                vec![leaf(7, 7, (12, 15), &[13, 14], false)],
            ),
        ]);
        let lhs = source(vec![
            leaf(8, 1, (0, 3), &[0], false),
            leaf(9, 9, (3, 4), &[], false),
            leaf(10, 8, (4, 7), &[4, 5], true),
        ]);
        let counts = visible_counts(&Pairing::Both { lhs, rhs });
        assert_eq!(counts.added, 2 + 2 + 3);
        assert_eq!(counts.removed, 1);
    }

    #[test]
    fn a_missing_side_counts_nothing() {
        let rhs = source(vec![leaf(0, 1, (0, 1), &[0], false)]);
        let counts = visible_counts(&Pairing::RightOnly { rhs });
        assert_eq!(counts.added, 1);
        assert_eq!(counts.removed, 0);
    }
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
            Options { syntax: false },
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
