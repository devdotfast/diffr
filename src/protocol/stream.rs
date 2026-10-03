//! The stdout stream: manifest, one record per file as it finishes, footer.
//!
//! Each file is diffed, projected, and shaped by the plugins
//! before its record is written; `stats.visible` is recounted after them.
use super::project::{self, Inputs};
use super::{
    Diff, Event, FileChange, LineRange, Node, Outcome, Problem, Region, Snapshot, Source,
    StructuralChanges, SyntaxSpan, Visibility, VERSION,
};
use crate::engine::QueryConflict;
use crate::git::{DiffSession, Differ, FileError, LoadedFile, PendingFile};
use crate::hash::DftHashSet;
use crate::pairing::Pairing;
use crate::plugin::{MutationFailed, Pipeline};
use crate::summary::{DiffResult, FileContent, FileFormat};
use std::io::{BufWriter, Write};
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use tokio::task::JoinSet;

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

/// Blocking workers load sources; `jobs` Rayon workers compute; async plugin
/// chains finish independently of the writer. Completed records queue in memory.
pub(crate) fn write(
    runtime: &tokio::runtime::Runtime,
    session: DiffSession,
    jobs: usize,
    pipeline: Arc<Pipeline>,
    options: Options,
    output: &mut impl Write,
) -> anyhow::Result<Ended> {
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .thread_name(|index| format!("diffr-worker-{index}"))
        .build()?;
    write_output(
        runtime,
        |sender| produce(session, Arc::new(pool), pipeline, options, sender),
        output,
    )
}

/// Run production on the application runtime while the caller writes records.
fn write_output<F: std::future::Future<Output = anyhow::Result<()>> + Send + 'static>(
    runtime: &tokio::runtime::Runtime,
    produce: impl FnOnce(Sender<Event>) -> F,
    output: &mut impl Write,
) -> anyhow::Result<Ended> {
    let (sender, receiver) = channel();
    let worker = runtime.spawn(produce(sender));
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
            serde_json::to_writer(&mut output, &event)?;
            output.write_all(b"\n")?;
            output.flush()?;
        }
        Ok(ended)
    })();
    // Stop outstanding file tasks when output fails.
    drop(receiver);
    if result.is_err() {
        worker.abort();
    }
    let joined = runtime.block_on(worker);
    let ended = result?;
    // Aborted only when output failed, which returned above.
    joined.expect("the diff task does not panic")?;
    Ok(ended)
}

async fn produce(
    session: DiffSession,
    pool: Arc<rayon::ThreadPool>,
    pipeline: Arc<Pipeline>,
    options: Options,
    sender: Sender<Event>,
) -> anyhow::Result<()> {
    sender.send(Event::Start {
        version: VERSION,
        lhs: Snapshot::from(&session.comparison.before),
        rhs: Snapshot::from(&session.comparison.after),
        files: session
            .file_manifest()
            .iter()
            .map(crate::git::FileChange::manifest_entry)
            .collect(),
    })?;
    let (differ, files) = session.into_files();
    let differ = Arc::new(differ);
    let mut pending = JoinSet::new();
    for file in files {
        pending.spawn(process_file(
            file,
            differ.clone(),
            pool.clone(),
            pipeline.clone(),
            options,
        ));
    }
    let (mut succeeded, mut failed) = (0, 0);
    let mut aborted = None;
    while let Some(result) = pending.join_next().await {
        let (file, result) = result.expect("a file task does not panic");
        let (visibility, outcome) = match result {
            Ok(result) => result,
            Err(error) => {
                failed += 1;
                aborted = Some(wire_error(&error));
                break;
            }
        };
        match &outcome {
            Outcome::Diff { .. } => succeeded += 1,
            Outcome::Error { .. } => failed += 1,
        }
        sender.send(Event::File {
            file: file.sides,
            visibility,
            outcome,
        })?;
    }
    sender.send(Event::Complete {
        succeeded,
        failed,
        aborted,
    })?;
    Ok(())
}

async fn process_file(
    pending: PendingFile,
    differ: Arc<Differ>,
    pool: Arc<rayon::ThreadPool>,
    pipeline: Arc<Pipeline>,
    options: Options,
) -> (
    crate::git::FileChange,
    anyhow::Result<(Visibility, Outcome)>,
) {
    let file = pending.file.clone();
    // Only blocking workers touch the repository or filesystem.
    let loading = differ.clone();
    let loaded = tokio::task::spawn_blocking(move || loading.load(pending))
        .await
        .expect("loading a file does not panic");
    let projected = match loaded {
        Ok(loaded) => {
            let (send, receive) = tokio::sync::oneshot::channel();
            pool.spawn(move || {
                // Detached Rayon panics retain Rayon's fatal default policy.
                let result = diffed(&differ, &loaded, options);
                let _ = send.send(result);
            });
            receive.await.expect("Rayon task returns its result")
        }
        Err(error) => Err(error),
    };
    let result = match projected {
        Ok((entry, diff)) => present(&pipeline, &entry, file.hidden.as_deref(), diff)
            .await
            .map(|(visibility, diff)| (visibility, Outcome::Diff { diff })),
        Err(error) => Ok((
            Visibility::default(),
            Outcome::Error {
                error: wire_error(&error),
            },
        )),
    };
    (file, result)
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
fn diffed(
    differ: &Differ,
    loaded: &LoadedFile,
    options: Options,
) -> anyhow::Result<(FileChange, Diff)> {
    let result = differ.diff(loaded)?;
    let syntax = match options.syntax {
        true => syntax_spans(&result, &differ.params),
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

/// Run the plugins on a diff and recount what stays visible: the file as it
/// is presented. A file the classifier hid runs no plugin and is shown
/// collapsed behind its reason. A binary diff has no text and no regions, so
/// only moves on the file apply to it. `Err` is a run-level failure.
async fn present(
    pipeline: &Pipeline,
    entry: &FileChange,
    hidden: Option<&str>,
    diff: Diff,
) -> anyhow::Result<(Visibility, Diff)> {
    let hide = |reason: &str| Visibility {
        collapsed: true,
        label: reason.to_owned(),
    };
    match diff {
        Diff::Text {
            sides, mut stats, ..
        } => {
            let (sides, visibility) = match hidden {
                Some(reason) => (sides, hide(reason)),
                None => pipeline.run(entry, sides).await?,
            };
            let coverage = change_coverage(&sides);
            stats.visible = coverage.initially_visible.counts();
            Ok((
                visibility,
                Diff::Text {
                    sides,
                    stats,
                    structural_changes: coverage.all,
                },
            ))
        }
        Diff::Binary { sides } => {
            let visibility = match hidden {
                Some(reason) => hide(reason),
                None => {
                    let empty = sides.clone().map(|_| Source {
                        text: String::new(),
                        syntax: Vec::new(),
                        regions: Vec::new(),
                    });
                    pipeline.run(entry, empty).await?.1
                }
            };
            Ok((visibility, Diff::Binary { sides }))
        }
    }
}

/// A standalone two-path comparison through the same three records.
pub(crate) fn write_file(
    runtime: &tokio::runtime::Runtime,
    before: &str,
    after: &str,
    sizes: (u64, u64),
    compute: impl FnOnce() -> Result<DiffResult, QueryConflict> + Send + 'static,
    params: Arc<crate::config::Params>,
    pipeline: Arc<Pipeline>,
    options: Options,
    output: &mut impl Write,
) -> anyhow::Result<Ended> {
    let file = crate::git::FileChange::standalone(before, after);
    let entry = file.manifest_entry();
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
    write_output(
        runtime,
        move |sender| async move {
            sender.send(start)?;
            let (send, receive) = tokio::sync::oneshot::channel();
            let sides = file.sides.clone();
            rayon::spawn(move || {
                let projected = compute().map(|result| {
                    project::diff(
                        &result,
                        Inputs {
                            file: &sides,
                            sizes,
                            syntax: if options.syntax {
                                syntax_spans(&result, &params)
                            } else {
                                (Vec::new(), Vec::new())
                            },
                        },
                    )
                });
                let _ = send.send(projected);
            });
            let result = match receive.await? {
                Ok(diff) => present(&pipeline, &entry, None, diff)
                    .await
                    .map(|(visibility, diff)| (visibility, Outcome::Diff { diff })),
                Err(conflict) => Ok((
                    Visibility::default(),
                    Outcome::Error {
                        error: wire_error(&conflict.into()),
                    },
                )),
            };
            let (succeeded, failed, aborted) = match result {
                Ok((visibility, outcome)) => {
                    let failed = matches!(outcome, Outcome::Error { .. });
                    sender.send(Event::File {
                        file: file.sides,
                        visibility,
                        outcome,
                    })?;
                    (u32::from(!failed), u32::from(failed), None)
                }
                Err(error) => (0, 1, Some(wire_error(&error))),
            };
            sender.send(Event::Complete {
                succeeded,
                failed,
                aborted,
            })?;
            Ok(())
        },
        output,
    )
}

/// Collect complete and default-visible coverage together. A paired leaf counts
/// only lines carrying changed spans; every line of an unpaired leaf counts,
/// including blank lines. Visibility never removes lines from `all`.
struct ChangeCoverage {
    all: StructuralChanges,
    initially_visible: StructuralChanges,
}

fn change_coverage(sides: &Pairing<Source>) -> ChangeCoverage {
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
    fn collect(
        regions: &[Region],
        other: &DftHashSet<u32>,
        hidden: bool,
        all: &mut Vec<LineRange>,
        visible: &mut Vec<LineRange>,
    ) {
        for region in regions {
            let hidden = hidden || region.visibility.collapsed;
            match &region.node {
                Node::Leaf {
                    alignment_id,
                    changed,
                } => {
                    let start = all.len();
                    if other.contains(alignment_id) {
                        all.extend(changed.iter().map(|span| [span.line, span.line + 1]));
                    } else {
                        let lines = region.range.lines();
                        all.push([lines.start, lines.end]);
                    }
                    if !hidden {
                        visible.extend_from_slice(&all[start..]);
                    }
                }
                Node::Fold { children } => collect(children, other, hidden, all, visible),
            }
        }
    }
    fn side(source: Option<&Source>, other: Option<&Source>) -> (Vec<LineRange>, Vec<LineRange>) {
        let mut paired = DftHashSet::default();
        if let Some(other) = other {
            alignments(&other.regions, &mut paired);
        }
        let (mut all, mut visible) = (Vec::new(), Vec::new());
        if let Some(source) = source {
            collect(&source.regions, &paired, false, &mut all, &mut visible);
        }
        (coalesce(all), coalesce(visible))
    }
    let (lhs, rhs) = match sides {
        Pairing::Both { lhs, rhs } => (Some(lhs), Some(rhs)),
        Pairing::LeftOnly { lhs } => (Some(lhs), None),
        Pairing::RightOnly { rhs } => (None, Some(rhs)),
    };
    let (base, visible_base) = side(lhs, rhs);
    let (head, visible_head) = side(rhs, lhs);
    ChangeCoverage {
        all: StructuralChanges { base, head },
        initially_visible: StructuralChanges {
            base: visible_base,
            head: visible_head,
        },
    }
}

/// Compact spans and whole-leaf ranges without allocating one entry per source line.
fn coalesce(mut ranges: Vec<LineRange>) -> Vec<LineRange> {
    ranges.sort_unstable();
    let mut merged: Vec<LineRange> = Vec::new();
    for [start, end] in ranges {
        if start >= end {
            continue;
        }
        if let Some(last) = merged.last_mut() {
            if start <= last[1] {
                last[1] = last[1].max(end);
                continue;
            }
        }
        merged.push([start, end]);
    }
    merged
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
            relations: Vec::new(),
            tags: vec![],
            visibility: Visibility {
                collapsed,
                label: String::new(),
            },
            node: Node::Leaf {
                alignment_id: alignment,
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
            relations: Vec::new(),
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
        let coverage = change_coverage(&Pairing::Both { lhs, rhs });
        assert_eq!(coverage.all.head, vec![[0, 2], [4, 15]]);
        assert_eq!(coverage.all.base, vec![[0, 1], [4, 7]]);
        let counts = coverage.initially_visible.counts();
        assert_eq!(counts.added, 2 + 2 + 3);
        assert_eq!(counts.removed, 1);
    }

    #[test]
    fn changing_fold_visibility_never_changes_complete_coverage() {
        let lhs = source(vec![leaf(1, 7, (0, 3), &[], false)]);
        let rhs = source(vec![fold(
            2,
            (0, 3),
            true,
            vec![fold(
                3,
                (0, 3),
                false,
                vec![leaf(4, 7, (0, 3), &[2, 0, 0], false)],
            )],
        )]);
        let mut sides = Pairing::Both { lhs, rhs };
        let hidden = change_coverage(&sides);
        assert_eq!(hidden.all.head, vec![[0, 1], [2, 3]]);
        assert!(hidden.all.base.is_empty()); // Added tokens do not imply removed tokens.
        assert_eq!(hidden.initially_visible.counts().added, 0);
        if let Pairing::Both { rhs, .. } = &mut sides {
            rhs.regions[0].visibility.collapsed = false;
        }
        let opened = change_coverage(&sides);
        assert_eq!(opened.all, hidden.all);
        assert_eq!(opened.initially_visible, opened.all);
    }

    #[test]
    fn deleted_blank_lines_and_empty_files_have_complete_coverage() {
        let lhs = source(vec![leaf(1, 0, (0, 2), &[], true)]);
        let deleted = change_coverage(&Pairing::LeftOnly { lhs });
        assert_eq!(deleted.all.base, vec![[0, 2]]);
        assert!(deleted.all.head.is_empty());
        assert_eq!(deleted.initially_visible.counts().removed, 0);
        let empty = change_coverage(&Pairing::RightOnly {
            rhs: source(vec![]),
        });
        assert_eq!(empty.all, StructuralChanges::default());
    }

    #[test]
    fn a_missing_side_counts_nothing() {
        let rhs = source(vec![leaf(0, 1, (0, 1), &[0], false)]);
        let counts = change_coverage(&Pairing::RightOnly { rhs })
            .initially_visible
            .counts();
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
        let params = Arc::new(params);
        let compute_params = params.clone();
        let ended = write_file(
            crate::test_runtime(),
            "a.rs",
            "b.rs",
            (0, 0),
            move || {
                DiffResult::try_from_sources_with_params(
                    "src/lib.rs",
                    "fn f() {\n    one();\n}\n",
                    "fn f() {\n    two();\n}\n",
                    &compute_params,
                )
            },
            params,
            Arc::new(
                Pipeline::from_config(
                    &crate::config::Config::from_toml("[plugins]\norder = []\n").unwrap(),
                    std::path::Path::new("."),
                    std::num::NonZeroUsize::MIN,
                )
                .unwrap(),
            ),
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

#[cfg(test)]
mod output_tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn production_finishes_while_the_writer_is_blocked() {
        struct BlockedWriter {
            started: Option<mpsc::Sender<()>>,
            finished: mpsc::Receiver<()>,
            bytes: Vec<u8>,
        }
        impl Write for BlockedWriter {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if let Some(started) = self.started.take() {
                    started.send(()).unwrap();
                    self.finished
                        .recv_timeout(Duration::from_secs(10))
                        .expect("production must finish while the writer is blocked");
                }
                self.bytes.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let (started, writing) = mpsc::channel();
        let (finished, completed) = mpsc::channel();
        let mut output = BlockedWriter {
            started: Some(started),
            finished: completed,
            bytes: vec![],
        };
        write_output(
            crate::test_runtime(),
            move |sender| async move {
                let event = || Event::Complete {
                    succeeded: 0,
                    failed: 0,
                    aborted: None,
                };
                sender.send(event())?;
                tokio::task::spawn_blocking(move || writing.recv_timeout(Duration::from_secs(10)))
                    .await
                    .expect("waiting for the writer does not panic")?;
                // These records are produced only after the writer blocks. A bounded
                // queue, or polling this future from the writer, would deadlock here.
                for _ in 0..100 {
                    sender.send(event())?;
                }
                finished.send(())?;
                Ok(())
            },
            &mut output,
        )
        .unwrap();
        assert_eq!(
            output.bytes.iter().filter(|&&byte| byte == b'\n').count(),
            101
        );
    }
}
