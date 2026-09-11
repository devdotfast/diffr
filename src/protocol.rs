//! The NDJSON wire protocol between diffr and its frontends.
//!
//! One `Event` per line: a `start` header, one `file` record per changed
//! file in completion order, and a `complete` footer. Every enum on the
//! wire is internally tagged with a `type` (or `kind`) key in snake_case.
//! Optional, empty, and default fields are omitted, never `null`. Consumers
//! ignore unknown fields and tolerate unknown enum strings.
//!
//! Coordinates: lines are 0-based and split on `\n` only, so an empty file
//! has zero lines and a file without a trailing newline still counts its
//! last line. Columns are 0-based byte offsets into the UTF-8 text on the
//! wire. All ranges are half-open.
//!
//! Sides are always `lhs` (before) and `rhs` (after). A `Pairing` says which
//! sides exist and serializes by presence: `{lhs, rhs}`, `{lhs}`, or `{rhs}`.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// The current wire version. Changes within a version are additive.
pub(crate) const VERSION: u32 = 2;

// ── stream ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub(crate) enum Event {
    /// The header. Sent once, before any result, so a frontend can lay out
    /// every file up front.
    Start {
        version: u32,
        lhs: Snapshot,
        rhs: Snapshot,
        files: Vec<FileChange>,
    },
    /// One result. `file` is byte-identical to the manifest entry it
    /// answers; the path pair is the identity.
    File {
        file: Pairing<FileRef>,
        #[serde(flatten)]
        outcome: Outcome,
    },
    /// The footer. `aborted` is present when a run-level failure stopped
    /// the comparison early; every file already emitted stays valid.
    Complete {
        succeeded: u32,
        failed: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        aborted: Option<Error>,
    },
}

/// Exactly one of `diff` or `error` appears on a `file` record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum Outcome {
    Diff { diff: Diff },
    Error { error: Error },
}

/// What one end of the comparison is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Snapshot {
    Revision {
        rev: String,
    },
    Index,
    WorkingTree,
    EmptyTree,
    /// A file on disk, for `--no-index`.
    Path {
        path: String,
    },
}

/// The one error shape, used for a file failure, a run abort, and a
/// structural fallback. `code` is an open snake_case set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Error {
    pub(crate) code: String,
    pub(crate) message: String,
}

/// Which sides a thing exists on. Serializes by presence.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Pairing<T> {
    Both { lhs: T, rhs: T },
    LeftOnly { lhs: T },
    RightOnly { rhs: T },
}

impl<T> Pairing<T> {
    pub(crate) fn lhs(&self) -> Option<&T> {
        match self {
            Self::Both { lhs, .. } | Self::LeftOnly { lhs } => Some(lhs),
            Self::RightOnly { .. } => None,
        }
    }

    pub(crate) fn rhs(&self) -> Option<&T> {
        match self {
            Self::Both { rhs, .. } | Self::RightOnly { rhs } => Some(rhs),
            Self::LeftOnly { .. } => None,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct PairingRepr<T> {
    #[serde(skip_serializing_if = "Option::is_none")]
    lhs: Option<T>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rhs: Option<T>,
}

impl<T: Serialize> Serialize for Pairing<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let repr = match self {
            Self::Both { lhs, rhs } => PairingRepr {
                lhs: Some(lhs),
                rhs: Some(rhs),
            },
            Self::LeftOnly { lhs } => PairingRepr {
                lhs: Some(lhs),
                rhs: None,
            },
            Self::RightOnly { rhs } => PairingRepr {
                lhs: None,
                rhs: Some(rhs),
            },
        };
        repr.serialize(serializer)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Pairing<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match PairingRepr::deserialize(deserializer)? {
            PairingRepr {
                lhs: Some(lhs),
                rhs: Some(rhs),
            } => Ok(Self::Both { lhs, rhs }),
            PairingRepr {
                lhs: Some(lhs),
                rhs: None,
            } => Ok(Self::LeftOnly { lhs }),
            PairingRepr {
                lhs: None,
                rhs: Some(rhs),
            } => Ok(Self::RightOnly { rhs }),
            PairingRepr {
                lhs: None,
                rhs: None,
            } => Err(D::Error::custom("a pairing needs at least one side")),
        }
    }
}

// ── manifest entry ────────────────────────────────────────────────────────

/// One changed file, known before any diffing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct FileChange {
    /// `LeftOnly` is a deletion, `RightOnly` an addition.
    pub(crate) file: Pairing<FileRef>,
    pub(crate) status: FileStatus,
    /// `source`, `test`, `generated`, `docs`, or a repository's own class,
    /// from git attributes and built-in path rules.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) category: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) language: Option<String>,
    #[serde(default, skip_serializing_if = "Visibility::is_default")]
    pub(crate) visibility: Visibility,
}

/// libgit2's delta status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FileStatus {
    Added,
    Deleted,
    Modified,
    Renamed,
    Copied,
    TypeChanged,
}

/// One side of a git delta.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct FileRef {
    pub(crate) path: String,
    pub(crate) oid: String,
    /// Git's octal mode text, e.g. `100644`.
    pub(crate) mode: String,
}

/// How a file or region starts out. `label` is shown while collapsed: a
/// reason for a file, a placeholder or pseudocode summary for a region.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub(crate) struct Visibility {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) collapsed: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(crate) label: String,
}

impl Visibility {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

// ── per-file result ───────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum Diff {
    Text {
        #[serde(flatten)]
        sides: Pairing<Source>,
        stats: Stats,
        /// Present when tree-sitter did not run and this is a line diff:
        /// no folds, and `stats.structural` is absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fallback: Option<Error>,
    },
    /// Either side being binary makes the whole diff binary.
    Binary {
        #[serde(flatten)]
        sides: Pairing<BinaryRef>,
    },
}

/// One side's text, colors, and regions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Source {
    pub(crate) text: String,
    /// Every token with its tree-sitter capture name. Per line, sorted,
    /// non-overlapping. Empty unless the run asked for syntax.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) syntax: Vec<SyntaxSpan>,
    /// The largest regions, in order. Leaves tile the file.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) regions: Vec<Region>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct BinaryRef {
    pub(crate) size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SyntaxSpan {
    pub(crate) line: u32,
    pub(crate) start_column: u32,
    pub(crate) end_column: u32,
    /// A tree-sitter capture name such as `keyword` or `function.method`.
    pub(crate) capture: String,
}

/// A range on one side. The same `id` on the other side is its
/// counterpart. Ids appear at most once per side. A paired leaf has the
/// same line count on both sides and its rows pair line for line; a paired
/// leaf whose counterpart is behind the reading cursor is a move, and the
/// frontend chooses how to show it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Region {
    pub(crate) id: u32,
    pub(crate) start: SourcePos,
    pub(crate) end: SourcePos,
    /// `body`, `import`, `test`, `unchanged`, or hook-supplied tags.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) tags: Vec<String>,
    /// A collapsed leaf is a context gap. A collapsed fold is a folded body.
    #[serde(default, skip_serializing_if = "Visibility::is_default")]
    pub(crate) visibility: Visibility,
    #[serde(flatten)]
    pub(crate) node: Node,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum Node {
    /// Tiles the file. `changed` holds the byte ranges painted as changed
    /// within it; a fully new line carries one span covering it.
    Leaf {
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        changed: Vec<Span>,
    },
    /// A foldable region. Its range is the hull of its children.
    Fold { children: Vec<Region> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Span {
    pub(crate) line: u32,
    pub(crate) start_column: u32,
    pub(crate) end_column: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SourcePos {
    pub(crate) line: u32,
    pub(crate) column: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Stats {
    /// Lines with any byte change.
    pub(crate) textual: LineCounts,
    /// Lines with a syntactic change. Absent when `fallback` is present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) structural: Option<LineCounts>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LineCounts {
    pub(crate) added: u32,
    pub(crate) removed: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pos(line: u32, column: u32) -> SourcePos {
        SourcePos { line, column }
    }

    fn leaf(id: u32, start: u32, end: u32, changed: Vec<Span>) -> Region {
        Region {
            id,
            start: pos(start, 0),
            end: pos(end, 0),
            tags: vec![],
            visibility: Visibility::default(),
            node: Node::Leaf { changed },
        }
    }

    /// A paired function with one changed word: `fn f() { 1 }` became
    /// `fn f() { 1 + 2 }`.
    fn example_file() -> Event {
        let file_ref = |oid: &str| FileRef {
            path: "src/lib.rs".to_owned(),
            oid: oid.to_owned(),
            mode: "100644".to_owned(),
        };
        let side = |text: &str, changed: Vec<Span>| Source {
            text: text.to_owned(),
            syntax: vec![],
            regions: vec![Region {
                id: 1,
                start: pos(0, 0),
                end: pos(3, 0),
                tags: vec!["body".to_owned()],
                visibility: Visibility::default(),
                node: Node::Fold {
                    children: vec![
                        leaf(2, 0, 1, vec![]),
                        leaf(3, 1, 2, changed),
                        leaf(4, 2, 3, vec![]),
                    ],
                },
            }],
        };
        Event::File {
            file: Pairing::Both {
                lhs: file_ref("3b18e5"),
                rhs: file_ref("9be2c1"),
            },
            outcome: Outcome::Diff {
                diff: Diff::Text {
                    sides: Pairing::Both {
                        lhs: side("fn f() {\n    1\n}\n", vec![]),
                        rhs: side(
                            "fn f() {\n    1 + 2\n}\n",
                            vec![Span {
                                line: 1,
                                start_column: 5,
                                end_column: 9,
                            }],
                        ),
                    },
                    stats: Stats {
                        textual: LineCounts {
                            added: 1,
                            removed: 1,
                        },
                        structural: Some(LineCounts {
                            added: 1,
                            removed: 0,
                        }),
                    },
                    fallback: None,
                },
            },
        }
    }

    #[test]
    fn file_record_serializes_to_the_documented_shape() {
        let region = |changed: serde_json::Value| {
            let mut middle = json!({"id": 3, "kind": "leaf", "start": {"line": 1, "column": 0}, "end": {"line": 2, "column": 0}});
            if let Some(spans) = changed.as_array().filter(|spans| !spans.is_empty()) {
                middle["changed"] = json!(spans);
            }
            json!({
                "id": 1, "kind": "fold", "tags": ["body"],
                "start": {"line": 0, "column": 0}, "end": {"line": 3, "column": 0},
                "children": [
                    {"id": 2, "kind": "leaf", "start": {"line": 0, "column": 0}, "end": {"line": 1, "column": 0}},
                    middle,
                    {"id": 4, "kind": "leaf", "start": {"line": 2, "column": 0}, "end": {"line": 3, "column": 0}},
                ],
            })
        };
        let expected = json!({
            "type": "file",
            "file": {
                "lhs": {"path": "src/lib.rs", "oid": "3b18e5", "mode": "100644"},
                "rhs": {"path": "src/lib.rs", "oid": "9be2c1", "mode": "100644"},
            },
            "diff": {
                "type": "text",
                "lhs": {"text": "fn f() {\n    1\n}\n", "regions": [region(json!([]))]},
                "rhs": {"text": "fn f() {\n    1 + 2\n}\n",
                        "regions": [region(json!([{"line": 1, "start_column": 5, "end_column": 9}]))]},
                "stats": {"textual": {"added": 1, "removed": 1}, "structural": {"added": 1, "removed": 0}},
            },
        });
        assert_eq!(serde_json::to_value(example_file()).unwrap(), expected);
    }

    #[test]
    fn every_event_round_trips() {
        let events = vec![
            Event::Start {
                version: VERSION,
                lhs: Snapshot::Revision {
                    rev: "main".to_owned(),
                },
                rhs: Snapshot::WorkingTree,
                files: vec![FileChange {
                    file: Pairing::RightOnly {
                        rhs: FileRef {
                            path: "gen/schema.json".to_owned(),
                            oid: "0e1f2a".to_owned(),
                            mode: "100644".to_owned(),
                        },
                    },
                    status: FileStatus::Added,
                    category: Some("generated".to_owned()),
                    language: None,
                    visibility: Visibility {
                        collapsed: true,
                        label: "Generated file · hidden by default".to_owned(),
                    },
                }],
            },
            example_file(),
            Event::File {
                file: Pairing::LeftOnly {
                    lhs: FileRef {
                        path: "old.bin".to_owned(),
                        oid: "aaaaaa".to_owned(),
                        mode: "100644".to_owned(),
                    },
                },
                outcome: Outcome::Diff {
                    diff: Diff::Binary {
                        sides: Pairing::LeftOnly {
                            lhs: BinaryRef { size: 4096 },
                        },
                    },
                },
            },
            Event::File {
                file: Pairing::Both {
                    lhs: FileRef {
                        path: "a.txt".to_owned(),
                        oid: "bbbbbb".to_owned(),
                        mode: "100644".to_owned(),
                    },
                    rhs: FileRef {
                        path: "a.txt".to_owned(),
                        oid: "cccccc".to_owned(),
                        mode: "100644".to_owned(),
                    },
                },
                outcome: Outcome::Error {
                    error: Error {
                        code: "not_utf8".to_owned(),
                        message: "a.txt is not valid UTF-8".to_owned(),
                    },
                },
            },
            Event::Complete {
                succeeded: 2,
                failed: 1,
                aborted: Some(Error {
                    code: "hook_failed".to_owned(),
                    message: "summarizer returned 503 after 4 attempts".to_owned(),
                }),
            },
        ];
        for event in events {
            let line = serde_json::to_string(&event).unwrap();
            assert_eq!(
                serde_json::from_str::<Event>(&line).unwrap(),
                event,
                "{line}"
            );
        }
    }

    #[test]
    fn defaults_are_omitted() {
        let line = serde_json::to_string(&example_file()).unwrap();
        for absent in [
            "null",
            "visibility",
            "collapsed",
            "syntax",
            "fallback",
            "\"changed\":[]",
        ] {
            assert!(!line.contains(absent), "{absent} appeared in {line}");
        }
    }

    #[test]
    fn a_pairing_needs_a_side() {
        let error = serde_json::from_value::<Pairing<u32>>(json!({})).unwrap_err();
        assert!(error.to_string().contains("at least one side"));
        assert_eq!(
            serde_json::from_value::<Pairing<u32>>(json!({"rhs": 7})).unwrap(),
            Pairing::RightOnly { rhs: 7 }
        );
    }
}
