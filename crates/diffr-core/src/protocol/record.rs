//! One file's record: tag it, diff it, project it, and let the plugins shape
//! it, then recount `stats.visible`. The stream ([`super::stream`]) writes
//! these records for a repository's files; a caller with the two sources and
//! no repository, such as the browser build, makes one with
//! [`file_from_sources`].
use super::project::{self, Inputs};
use super::{
    Diff, Event, FileChange, FileRef, FileStatus, LineRange, Node, Outcome, Problem, Region,
    Source, StructuralChanges, SyntaxSpan, Visibility,
};
use crate::engine::QueryConflict;
use crate::hash::DftHashSet;
use crate::pairing::Pairing;
use crate::params::{DiffOptions, Params};
use crate::plugin::{MutationFailed, Pipeline};
use crate::summary::{DiffResult, FallbackCause, FileContent, FileFormat};
use crate::tags::{self, Prefix};
use std::fmt;

/// Runtime choices that shape every file record.
#[derive(Clone, Copy)]
pub struct Options {
    /// Emit every token's capture name (`--syntax`).
    pub syntax: bool,
    /// Opt-in v4 stream; v3 consumers continue to receive finished files.
    pub updates: bool,
}

/// An enrichment failure is local to this file's annotations.
pub fn enrich_event(pipeline: &Pipeline, entry: &FileChange, sides: &Pairing<Source>) -> Event {
    let (annotations, error) = match pipeline.enrich(entry, sides) {
        Ok(annotations) => (annotations, None),
        Err(error) => (
            Vec::new(),
            Some(Problem {
                code: "enrichment_failed".into(),
                message: format!("{error:#}"),
            }),
        ),
    };
    Event::Annotations {
        file: entry.file.clone(),
        annotations,
        error,
    }
}

/// Why one file could not be diffed. Loading attaches it to the error, and
/// the stream turns it into the record's `code`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileError {
    UnsupportedFileType,
    ReadFailed,
    NotUtf8,
    Unmerged,
}

impl FileError {
    pub fn code(self) -> &'static str {
        match self {
            Self::UnsupportedFileType => "unsupported_file_type",
            Self::ReadFailed => "read_failed",
            Self::NotUtf8 => "not_utf8",
            Self::Unmerged => "unmerged",
        }
    }
}

impl fmt::Display for FileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsupportedFileType => {
                "structural diffs currently require regular text files (not symlinks or submodules)"
            }
            Self::ReadFailed => "could not read the source",
            Self::NotUtf8 => "the source is not valid UTF-8",
            Self::Unmerged => {
                "unmerged index entry: resolve the conflict before requesting a structural diff"
            }
        })
    }
}

impl std::error::Error for FileError {}

/// The wire record for an error, built as it is written. The code comes from
/// the typed cause attached where the error arose; an error nothing
/// classified is `internal`.
pub fn wire_error(error: &anyhow::Error) -> Problem {
    if let Some(kind) = error.downcast_ref::<FileError>() {
        return Problem {
            code: kind.code().to_owned(),
            message: format!("{error:#}"),
        };
    }
    let code = if error.downcast_ref::<QueryConflict>().is_some() {
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

/// Highlight spans for both sides of a file in a known language. A line diff
/// that fell back because matching grew too large or the parse had too many
/// errors is still coloured; one over the byte limit, or generated, is not,
/// since highlighting it would cost what the limit was there to avoid.
pub fn syntax_spans(
    diff: &DiffResult,
    params: &crate::params::Params,
) -> (Vec<SyntaxSpan>, Vec<SyntaxSpan>) {
    let language = match &diff.file_format {
        FileFormat::SupportedLanguage(language) => *language,
        FileFormat::TextFallback {
            cause: FallbackCause::GraphLimit | FallbackCause::ParseErrorLimit,
            language: Some(language),
            ..
        } => *language,
        _ => return (Vec::new(), Vec::new()),
    };
    let parser = params.language(language).parser;
    let spans = |content: &FileContent| match content {
        FileContent::Text(src) => project::syntax_spans(src, parser),
        FileContent::Binary => Vec::new(),
    };
    (spans(&diff.lhs_src), spans(&diff.rhs_src))
}

/// Run the plugins on a diff and recount what stays visible. A binary diff has
/// no text and no regions, so only moves on the file apply to it. `Err`
/// is a run-level failure.
pub fn shape(
    pipeline: &Pipeline,
    entry: &FileChange,
    diff: Diff,
    updates: bool,
) -> anyhow::Result<(Visibility, Diff)> {
    match diff {
        Diff::Text {
            mut sides,
            mut stats,
            ..
        } => {
            let visibility = if updates {
                pipeline.prepare(entry, &mut sides)?
            } else {
                pipeline.run(entry, &mut sides)?
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
            let mut empty = sides.clone().map(|_| Source {
                text: String::new(),
                syntax: Vec::new(),
                regions: Vec::new(),
            });
            let visibility = if updates {
                pipeline.prepare(entry, &mut empty)?
            } else {
                pipeline.run(entry, &mut empty)?
            };
            Ok((visibility, Diff::Binary { sides }))
        }
    }
}

/// One file's record from the text of both sides, for a caller that has
/// the sources but no repository, such as the browser build. `file` and
/// `status` describe the change as git would; a side the change does not
/// have is `None`. The file goes through what a repository's files go
/// through: the bundled tags (by path, then by content), each plugin's
/// `classify`, the diff, and each plugin's `mutate`. Git attributes, which
/// need the repository, are not read. Returns the manifest entry and the
/// `file` record.
pub fn file_from_sources(
    file: Pairing<FileRef>,
    status: FileStatus,
    before: Option<&str>,
    after: Option<&str>,
    params: &Params,
    pipeline: &Pipeline,
    diff_options: &DiffOptions,
    options: Options,
) -> (FileChange, Event) {
    let path = match &file {
        Pairing::Both { rhs, .. } | Pairing::RightOnly { rhs } => rhs.path.clone(),
        Pairing::LeftOnly { lhs } => lhs.path.clone(),
    };
    let mut bundled = tags::from_path(&path);
    if !bundled.contains(tags::GENERATED) && tags::needs_content(&path) {
        if let Some(text) = after.or(before) {
            let complete = text.len() <= tags::PREFIX_BYTES;
            let mut end = text.len().min(tags::PREFIX_BYTES);
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let prefix = Prefix {
                text: &text[..end],
                complete,
            };
            if tags::generated_by_content(&path, &prefix) {
                bundled.insert(tags::GENERATED);
            }
        }
    }
    let mut entry = FileChange {
        file,
        status,
        tags: bundled.into_iter().map(str::to_owned).collect(),
    };
    let record = |entry: &FileChange, visibility, outcome| Event::File {
        file: entry.file.clone(),
        visibility,
        outcome,
    };
    let failed = |entry: &FileChange, error: anyhow::Error| {
        record(
            entry,
            Visibility::default(),
            Outcome::Error {
                error: wire_error(&error),
            },
        )
    };
    match pipeline.classify(&entry) {
        Ok(tags) => entry.tags = tags,
        Err(error) => {
            let event = failed(&entry, error);
            return (entry, event);
        }
    }
    let (lhs, rhs) = (before.unwrap_or(""), after.unwrap_or(""));
    let result = if lhs.contains('\0') || rhs.contains('\0') {
        Ok(DiffResult {
            file_format: FileFormat::Binary,
            lhs_src: FileContent::Binary,
            rhs_src: FileContent::Binary,
            lhs_positions: vec![],
            rhs_positions: vec![],
            lhs_folds: vec![],
            rhs_folds: vec![],
        })
    } else {
        let diff_options = DiffOptions {
            generated: entry.tags.iter().any(|tag| tag == tags::GENERATED),
            ..diff_options.clone()
        };
        DiffResult::from_sources_with_options(&path, lhs, rhs, params, &diff_options)
    };
    let event = match result {
        Err(conflict) => failed(&entry, conflict.into()),
        Ok(result) => {
            let projected = project::diff(
                &result,
                Inputs {
                    file: &entry.file,
                    sizes: (lhs.len() as u64, rhs.len() as u64),
                    syntax: match options.syntax {
                        true => syntax_spans(&result, params),
                        false => (Vec::new(), Vec::new()),
                    },
                },
            );
            match shape(pipeline, &entry, projected, options.updates) {
                Ok((visibility, diff)) => record(&entry, visibility, Outcome::Diff { diff }),
                Err(error) => failed(&entry, error),
            }
        }
    };
    (entry, event)
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
