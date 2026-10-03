//! Diff a comparison and write the record stream. Files are classified
//! first, then diffed on worker threads and written as each finishes.
use crate::config::Params;
use crate::engine::QueryConflict;
use crate::git::{self, FileError};
use crate::hash::DftHashSet;
use crate::options::DiffOptions;
use crate::pairing::Pairing;
use crate::plugin::{Classifier, MutationFailed, Pipeline};
use crate::protocol::project::{self, Inputs};
use crate::protocol::{
    Diff, Event, FileChange, LineRange, Node, Outcome, Problem, Region, Source, StructuralChanges,
    SyntaxSpan, Visibility, VERSION,
};
use crate::summary::{DiffResult, FallbackCause, FileContent, FileFormat};
use crate::tags;
use std::io::{BufWriter, Write};
use std::num::NonZeroUsize;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use tokio::task::JoinSet;

/// The command line's choices for one run, beyond the configuration.
#[derive(Clone, Copy)]
pub(crate) struct Options {
    /// Emit every token's capture name (`--syntax`).
    pub(crate) syntax: bool,
    /// Diff without comments (`--ignore-comments`).
    pub(crate) ignore_comments: bool,
}

/// What the stream ended with: how many files it listed, whether any
/// failed, and whether a run-level failure cut it short.
pub(crate) struct Ended {
    pub(crate) files: usize,
    pub(crate) failed: bool,
    pub(crate) aborted: bool,
}

/// Tag every file before any is diffed; the start record needs the tags.
pub(crate) fn classify(
    classifier: &mut Classifier,
    listing: &mut git::Listing,
) -> anyhow::Result<()> {
    let entries: Vec<FileChange> = listing
        .files
        .iter()
        .map(|file| file.change.manifest_entry())
        .collect();
    for (file, classified) in listing.files.iter_mut().zip(classifier.classify(&entries)?) {
        file.change.tags = classified.tags;
        file.change.hidden = classified.hidden;
    }
    Ok(())
}

/// Write the start record, each file as it finishes, and the footer.
/// Records queue without bound, so a slow reader never stalls diffing.
pub(crate) fn stream(
    runtime: &tokio::runtime::Runtime,
    listing: git::Listing,
    pipeline: Pipeline,
    params: Params,
    jobs: NonZeroUsize,
    options: Options,
    output: &mut impl Write,
) -> anyhow::Result<Ended> {
    let start = Event::Start {
        version: VERSION,
        lhs: listing.lhs,
        rhs: listing.rhs,
        files: listing
            .files
            .iter()
            .map(|file| file.change.manifest_entry())
            .collect(),
    };
    let files = listing.files;
    let count = files.len();
    let shared = Arc::new(Shared {
        pool: rayon::ThreadPoolBuilder::new()
            .num_threads(jobs.get())
            .thread_name(|index| format!("diffr-worker-{index}"))
            .build()?,
        pipeline,
        diff_options: params.diff.options(options.ignore_comments),
        params,
        syntax: options.syntax,
    });
    let (sender, receiver) = channel();
    let worker = runtime.spawn(produce(start, files, shared, sender));
    let mut output = BufWriter::new(output);
    let result: anyhow::Result<Ended> = (|| {
        let mut ended = Ended {
            files: count,
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

/// What every per-file task shares.
struct Shared {
    /// `jobs` threads for reading, diffing and projecting.
    pool: rayon::ThreadPool,
    pipeline: Pipeline,
    params: Params,
    diff_options: DiffOptions,
    syntax: bool,
}

/// The start record, each file's record as it finishes, then the footer. A
/// run-level failure stops the files and is reported in the footer.
async fn produce(
    start: Event,
    files: Vec<git::File>,
    shared: Arc<Shared>,
    sender: Sender<Event>,
) -> anyhow::Result<()> {
    sender.send(start)?;
    let mut pending = JoinSet::new();
    for file in files {
        pending.spawn(record(file, shared.clone()));
    }
    let (mut succeeded, mut failed) = (0, 0);
    let mut aborted = None;
    while let Some(record) = pending.join_next().await {
        let record = match record.expect("a file task does not panic") {
            Ok(record) => record,
            Err(error) => {
                failed += 1;
                aborted = Some(wire_error(&error));
                break;
            }
        };
        if let Event::File { outcome, .. } = &record {
            match outcome {
                Outcome::Diff { .. } => succeeded += 1,
                Outcome::Error { .. } => failed += 1,
            }
        }
        sender.send(record)?;
    }
    sender.send(Event::Complete {
        succeeded,
        failed,
        aborted,
    })?;
    Ok(())
}

/// One file's record. Its own failure is the record's error outcome; `Err` is
/// a run-level failure.
async fn record(file: git::File, shared: Arc<Shared>) -> anyhow::Result<Event> {
    let (send, receive) = tokio::sync::oneshot::channel();
    let change = file.change.clone();
    let projecting = shared.clone();
    shared.pool.spawn(move || {
        let _ = send.send(project(&file, &projecting));
    });
    let (visibility, outcome) = match receive.await.expect("Rayon task returns its result") {
        Ok(diff) => {
            let (visibility, diff) = present(
                &shared.pipeline,
                &change.manifest_entry(),
                change.hidden.as_deref(),
                diff,
            )
            .await?;
            (visibility, Outcome::Diff { diff })
        }
        Err(error) => (
            Visibility::default(),
            Outcome::Error {
                error: wire_error(&error),
            },
        ),
    };
    Ok(Event::File {
        file: change.sides,
        visibility,
        outcome,
    })
}

/// Read, diff and project one file: the blocking work. A fold query conflict
/// fails this file alone.
fn project(file: &git::File, shared: &Shared) -> anyhow::Result<Diff> {
    let (before, after) = file.read()?;
    let sizes = (before.len() as u64, after.len() as u64);
    // A binary file is a successful, size-only record.
    let result = if before.contains(&0) || after.contains(&0) {
        DiffResult {
            file_format: FileFormat::Binary,
            lhs_src: FileContent::Binary,
            rhs_src: FileContent::Binary,
            lhs_positions: vec![],
            rhs_positions: vec![],
            lhs_folds: vec![],
            rhs_folds: vec![],
        }
    } else {
        let before = std::str::from_utf8(&before).map_err(|_| FileError::NotUtf8)?;
        let after = std::str::from_utf8(&after).map_err(|_| FileError::NotUtf8)?;
        let change = &file.change;
        let options = DiffOptions {
            by_line: if change.tags.iter().any(|tag| tag == tags::GENERATED) {
                Some(FallbackCause::Generated)
            } else if change.hidden.is_some() {
                Some(FallbackCause::Hidden)
            } else {
                None
            },
            ..shared.diff_options.clone()
        };
        DiffResult::from_sources_with_options(
            change.path(),
            before,
            after,
            &shared.params,
            &options,
        )?
    };
    let syntax = syntax_spans(&result, shared);
    Ok(project::diff(
        &result,
        Inputs {
            file: &file.change.sides,
            sizes,
            syntax,
        },
    ))
}

/// The wire record for an error, with the code of its typed cause;
/// unclassified errors are `internal`.
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

/// Highlight spans for both sides of a structurally parsed file, when the run
/// asked for them. A file that fell back to a line diff has none.
fn syntax_spans(diff: &DiffResult, shared: &Shared) -> (Vec<SyntaxSpan>, Vec<SyntaxSpan>) {
    let FileFormat::SupportedLanguage(language) = &diff.file_format else {
        return (Vec::new(), Vec::new());
    };
    if !shared.syntax {
        return (Vec::new(), Vec::new());
    }
    let parser = shared.params.language(*language).parser;
    let spans = |content: &FileContent| match content {
        FileContent::Text(src) => project::syntax_spans(src, parser),
        FileContent::Binary => Vec::new(),
    };
    (spans(&diff.lhs_src), spans(&diff.rhs_src))
}

/// Run the plugins on a diff and recount what stays visible. A hidden file
/// runs no plugin and is shown collapsed; a binary diff has nothing to shape.
/// `Err` is a run-level failure.
async fn present(
    pipeline: &Pipeline,
    entry: &FileChange,
    hidden: Option<&str>,
    diff: Diff,
) -> anyhow::Result<(Visibility, Diff)> {
    let visibility = match hidden {
        Some(reason) => Visibility {
            collapsed: true,
            label: reason.to_owned(),
        },
        None => Visibility::default(),
    };
    match diff {
        Diff::Text {
            sides, mut stats, ..
        } => {
            let sides = match hidden {
                Some(_) => sides,
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
        Diff::Binary { sides } => Ok((visibility, Diff::Binary { sides })),
    }
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
