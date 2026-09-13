//! Bundle each function with its docstring, so they open and close together.
//!
//! A docstring is the run of comment lines just above a function, with only
//! blank lines and the function's own signature between them; in Python it
//! is instead a string that is the body's first statement. Its lines become
//! leaves tagged `docstring` whose `fold_state_id` is the function's. The
//! function's pairing across sides is untouched: a docstring leaf that is
//! paired keeps its bundle only when the other side bundles it with the
//! same function too. [`DocstringVisibility`] then collapses a docstring,
//! with an empty label, wherever its function starts collapsed.
use super::group::next_id;
use super::{collapse, comment_marker, ids, walk, walk_mut, FoldMutation};
use crate::hash::{DftHashMap, DftHashSet};
use crate::pairing::Pairing;
use crate::protocol::{FileChange, Node, Region, Source, SourcePos, SourceRange};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const TAG: &str = "docstring";

/// How many signature lines may sit between a docstring and the line a
/// function's body opens on.
const MAX_SIGNATURE_LINES: u32 = 12;

pub(crate) struct Docstrings;

impl FoldMutation for Docstrings {
    fn apply(&self, file: &FileChange, sides: &mut Pairing<Source>) -> anyhow::Result<()> {
        let language = file.language.as_deref();
        let mut next = next_id(sides);
        let (mut lhs, mut rhs) = sides_mut(sides);
        let lhs_docs = lhs
            .as_deref()
            .map(|source| usable(source, find(source, language)))
            .unwrap_or_default();
        let rhs_docs = rhs
            .as_deref()
            .map(|source| usable(source, find(source, language)))
            .unwrap_or_default();
        if lhs_docs.is_empty() && rhs_docs.is_empty() {
            return Ok(());
        }

        // Split every leaf at docstring edges, mirroring splits across
        // paired leaves so both sides keep equal-length pieces.
        let mut offsets: BTreeMap<u32, BTreeSet<u32>> = BTreeMap::new();
        for (source, docs) in [(lhs.as_deref(), &lhs_docs), (rhs.as_deref(), &rhs_docs)] {
            let Some(source) = source else { continue };
            let edges: BTreeSet<u32> = docs.iter().flat_map(|doc| [doc.start, doc.end]).collect();
            walk(&source.regions, &mut |region| {
                if let Node::Leaf { .. } = region.node {
                    let lines = region.range.lines();
                    for &edge in edges.range(lines.start + 1..lines.end) {
                        offsets
                            .entry(region.alignment_id)
                            .or_default()
                            .insert(edge - lines.start);
                    }
                }
            });
        }
        let mut fresh: BTreeMap<(u32, usize), u32> = BTreeMap::new();
        if let Some(lhs) = lhs.as_deref_mut() {
            split(&mut lhs.regions, &offsets, &mut fresh, &mut next);
        }
        if let Some(rhs) = rhs.as_deref_mut() {
            split(&mut rhs.regions, &offsets, &mut fresh, &mut next);
        }

        // Mark, then undo any paired docstring leaf whose partner disagrees.
        let lhs_marked = lhs
            .as_deref_mut()
            .map(|source| mark(source, &lhs_docs))
            .unwrap_or_default();
        let rhs_marked = rhs
            .as_deref_mut()
            .map(|source| mark(source, &rhs_docs))
            .unwrap_or_default();
        let lhs_ids = lhs
            .as_deref()
            .map(|source| ids(&source.regions))
            .unwrap_or_default();
        let rhs_ids = rhs
            .as_deref()
            .map(|source| ids(&source.regions))
            .unwrap_or_default();
        let mut revert: DftHashSet<u32> = DftHashSet::default();
        for (marked, other_marked, other_ids) in [
            (&lhs_marked, &rhs_marked, &rhs_ids),
            (&rhs_marked, &lhs_marked, &lhs_ids),
        ] {
            for (id, fold_state) in marked {
                if other_ids.contains(id) && other_marked.get(id) != Some(fold_state) {
                    revert.insert(*id);
                }
            }
        }
        if !revert.is_empty() {
            for source in [lhs, rhs].into_iter().flatten() {
                unmark(&mut source.regions, &revert);
            }
        }
        Ok(())
    }
}

/// Collapse each docstring, with an empty label, whose function starts
/// collapsed. Runs after every mutation that collapses functions.
pub(crate) struct DocstringVisibility;

impl FoldMutation for DocstringVisibility {
    fn apply(&self, _file: &FileChange, sides: &mut Pairing<Source>) -> anyhow::Result<()> {
        let (lhs, rhs) = sides_mut(sides);
        for source in [lhs, rhs].into_iter().flatten() {
            let mut collapsed: DftHashSet<u32> = DftHashSet::default();
            walk(&source.regions, &mut |region| {
                if is_function(region) && region.visibility.collapsed {
                    collapsed.insert(region.fold_state_id);
                }
            });
            walk_mut(&mut source.regions, &mut |region| {
                if is_docstring(region) && collapsed.contains(&region.fold_state_id) {
                    collapse(region, String::new());
                }
            });
        }
        Ok(())
    }
}

/// The docstring text for a function: its docstring lines on this side
/// with comment markers and quotes stripped, or `None` when there is none
/// or it says nothing.
pub(crate) fn text_for(source: &Source, fold_state_id: u32) -> Option<String> {
    let lines: Vec<&str> = source.text.split_terminator('\n').collect();
    let mut words = Vec::new();
    walk(&source.regions, &mut |region| {
        if is_docstring(region) && region.fold_state_id == fold_state_id {
            for line in region.range.lines() {
                if let Some(text) = lines.get(line as usize) {
                    let stripped = strip_markers(text);
                    if !stripped.is_empty() {
                        words.push(stripped.to_owned());
                    }
                }
            }
        }
    });
    let text = words.join(" ");
    (!text.is_empty()).then_some(text)
}

pub(crate) fn is_docstring(region: &Region) -> bool {
    region.tags.iter().any(|tag| tag == TAG)
}

fn is_function(region: &Region) -> bool {
    matches!(region.node, Node::Fold { .. }) && region.tags.iter().any(|tag| tag == "function")
}

/// A docstring's half-open line range and the function it belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Doc {
    fold_state_id: u32,
    start: u32,
    end: u32,
}

fn find(source: &Source, language: Option<&str>) -> Vec<Doc> {
    let lines: Vec<&str> = source.text.split_terminator('\n').collect();
    let mut docs = Vec::new();
    walk(&source.regions, &mut |region| {
        if !is_function(region) {
            return;
        }
        let opens = region.range.start.line;
        let found = if language == Some("Python") {
            python_docstring(&lines, opens)
        } else {
            comment_above(&lines, opens, comment_marker(language))
        };
        if let Some((start, end)) = found {
            docs.push(Doc {
                fold_state_id: region.fold_state_id,
                start,
                end,
            });
        }
    });
    docs
}

/// Drop docstrings that touch a leaf which already starts collapsed: those
/// lines are hidden already, and splitting a gap would break it up.
fn usable(source: &Source, docs: Vec<Doc>) -> Vec<Doc> {
    docs.into_iter()
        .filter(|doc| {
            let mut clear = true;
            walk(&source.regions, &mut |region| {
                if let Node::Leaf { .. } = region.node {
                    let lines = region.range.lines();
                    if lines.start < doc.end && doc.start < lines.end && region.visibility.collapsed
                    {
                        clear = false;
                    }
                }
            });
            clear
        })
        .collect()
}

/// Comment lines above the line a function's body opens on, skipping its
/// signature and then blank lines. A line that ends a statement or a block
/// stops the search, so a comment separated from the function by code does
/// not count.
fn comment_above(lines: &[&str], opens: u32, marker: &str) -> Option<(u32, u32)> {
    let is_comment = |line: &str| {
        let line = line.trim_start();
        line.starts_with(marker)
            || (marker == "//"
                && (line.starts_with("/*") || line.starts_with('*') || line.starts_with("*/")))
    };
    let is_blank = |line: &str| line.trim().is_empty();
    let ends_code = |line: &str| {
        let line = line.trim_end();
        line.ends_with(';') || line.ends_with('}') || line.ends_with('{')
    };
    let mut at = opens;
    // The signature: lines above the opening line that are not blank, not
    // comments, and do not end a statement or a block.
    let mut signature = 0;
    while at > 0 {
        let line = lines.get(at as usize - 1)?;
        if is_blank(line) || is_comment(line) || ends_code(line) || signature >= MAX_SIGNATURE_LINES
        {
            break;
        }
        at -= 1;
        signature += 1;
    }
    while at > 0
        && lines
            .get(at as usize - 1)
            .is_some_and(|line| is_blank(line))
    {
        at -= 1;
    }
    let end = at;
    while at > 0
        && lines
            .get(at as usize - 1)
            .is_some_and(|line| is_comment(line))
    {
        at -= 1;
    }
    (at < end).then_some((at, end))
}

/// A string that is the first statement of a Python body. The body fold
/// opens on its first statement line.
fn python_docstring(lines: &[&str], opens: u32) -> Option<(u32, u32)> {
    let first = lines.get(opens as usize)?.trim_start();
    let body = first.trim_start_matches(['r', 'R', 'u', 'U', 'b', 'B', 'f', 'F']);
    let quote = ["\"\"\"", "'''", "\"", "'"]
        .into_iter()
        .find(|quote| body.starts_with(quote))?;
    if quote.len() == 1 {
        return Some((opens, opens + 1));
    }
    if body[quote.len()..].contains(quote) {
        return Some((opens, opens + 1));
    }
    (opens as usize + 1..lines.len())
        .find(|&index| lines[index].contains(quote))
        .map(|index| (opens, index as u32 + 1))
}

fn strip_markers(line: &str) -> &str {
    let mut text = line.trim();
    for prefix in [
        "///", "//!", "//", "/**", "/*", "*/", "*", "#", "--", ";;", ";", "%",
    ] {
        if let Some(rest) = text.strip_prefix(prefix) {
            text = rest.trim();
            break;
        }
    }
    for quote in ["\"\"\"", "'''"] {
        text = text
            .trim_start_matches(quote)
            .trim_end_matches(quote)
            .trim();
    }
    text.trim_end_matches("*/").trim()
}

fn sides_mut(sides: &mut Pairing<Source>) -> (Option<&mut Source>, Option<&mut Source>) {
    match sides {
        Pairing::Both { lhs, rhs } => (Some(lhs), Some(rhs)),
        Pairing::LeftOnly { lhs } => (Some(lhs), None),
        Pairing::RightOnly { rhs } => (None, Some(rhs)),
    }
}

/// Cut leaves at the given offsets. The first piece keeps the leaf's ids;
/// later pieces take fresh ids shared by the two sides of a paired leaf.
fn split(
    regions: &mut Vec<Region>,
    offsets: &BTreeMap<u32, BTreeSet<u32>>,
    fresh: &mut BTreeMap<(u32, usize), u32>,
    next: &mut u32,
) {
    let mut out = Vec::with_capacity(regions.len());
    for mut region in regions.drain(..) {
        match &mut region.node {
            Node::Fold { children } => {
                split(children, offsets, fresh, next);
                out.push(region);
            }
            Node::Leaf { .. } => match offsets.get(&region.alignment_id) {
                Some(cuts) => out.extend(pieces(region, cuts, fresh, next)),
                None => out.push(region),
            },
        }
    }
    *regions = out;
}

fn pieces(
    region: Region,
    cuts: &BTreeSet<u32>,
    fresh: &mut BTreeMap<(u32, usize), u32>,
    next: &mut u32,
) -> Vec<Region> {
    let Node::Leaf { changed } = region.node else {
        unreachable!("only leaves are split");
    };
    let lines = region.range.lines();
    let at = |line: u32| SourcePos { line, column: 0 };
    let mut bounds: Vec<u32> = vec![lines.start];
    bounds.extend(cuts.iter().map(|cut| lines.start + cut));
    bounds.push(lines.end);
    bounds
        .windows(2)
        .enumerate()
        .map(|(index, window)| {
            let range = SourceRange {
                start: if index == 0 {
                    region.range.start
                } else {
                    at(window[0])
                },
                end: at(window[1]),
            };
            let id = if index == 0 {
                region.alignment_id
            } else {
                *fresh
                    .entry((region.alignment_id, index))
                    .or_insert_with(|| {
                        let id = *next;
                        *next += 1;
                        id
                    })
            };
            let span_lines = range.lines();
            Region {
                alignment_id: id,
                fold_state_id: if index == 0 { region.fold_state_id } else { id },
                range,
                tags: region.tags.clone(),
                visibility: region.visibility.clone(),
                node: Node::Leaf {
                    changed: changed
                        .iter()
                        .copied()
                        .filter(|span| span_lines.contains(&span.line))
                        .collect(),
                },
            }
        })
        .collect()
}

/// Tag the leaves inside each docstring and give them the function's fold
/// state. Returns alignment id to fold state id for every marked leaf.
fn mark(source: &mut Source, docs: &[Doc]) -> DftHashMap<u32, u32> {
    let mut marked = DftHashMap::default();
    walk_mut(&mut source.regions, &mut |region| {
        if let Node::Leaf { .. } = region.node {
            let lines = region.range.lines();
            if let Some(doc) = docs
                .iter()
                .find(|doc| doc.start <= lines.start && lines.end <= doc.end)
            {
                if !is_docstring(region) {
                    region.tags.push(TAG.to_owned());
                }
                region.fold_state_id = doc.fold_state_id;
                marked.insert(region.alignment_id, doc.fold_state_id);
            }
        }
    });
    marked
}

fn unmark(regions: &mut [Region], ids: &DftHashSet<u32>) {
    walk_mut(regions, &mut |region| {
        if ids.contains(&region.alignment_id) && is_docstring(region) {
            region.tags.retain(|tag| tag != TAG);
            region.fold_state_id = region.alignment_id;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutate::summarize::tests::project;

    fn docstrings(source: &Source) -> Vec<(u32, u32, u32)> {
        let mut out = Vec::new();
        walk(&source.regions, &mut |region| {
            if is_docstring(region) {
                let lines = region.range.lines();
                out.push((lines.start, lines.end, region.fold_state_id));
            }
        });
        out
    }

    fn function_state(source: &Source, header: &str) -> u32 {
        let lines: Vec<&str> = source.text.split_terminator('\n').collect();
        let mut found = None;
        walk(&source.regions, &mut |region| {
            if is_function(region) {
                let start = region.range.start.line as usize;
                if (start.saturating_sub(3)..=start).any(|line| lines[line].contains(header)) {
                    found.get_or_insert(region.fold_state_id);
                }
            }
        });
        found.expect("function fold")
    }

    #[test]
    fn rust_doc_and_line_comments_above_a_function_are_its_docstring() {
        let before = "fn keep() -> u32 {\n    1\n}\n";
        let after = "fn keep() -> u32 {\n    1\n}\n\n/// Adds one.\n/// Twice, really.\nfn add(\n    a: u32,\n) -> u32 {\n    a + 2\n}\n\n// Plain comment.\nfn sub(a: u32) -> u32 {\n    a - 1\n}\n";
        let (file, mut sides) = project("a.rs", before, after);
        Docstrings.apply(&file, &mut sides).unwrap();
        let (Pairing::Both { rhs, .. } | Pairing::RightOnly { rhs }) = &sides else {
            panic!("an after side");
        };
        let docs = docstrings(rhs);
        let add = function_state(rhs, "fn add(");
        let sub = function_state(rhs, "fn sub(");
        assert!(
            docs.iter().any(|&(s, e, f)| (s, e, f) == (4, 6, add)),
            "{docs:?}"
        );
        assert!(
            docs.iter().any(|&(s, e, f)| (s, e, f) == (12, 13, sub)),
            "{docs:?}"
        );
        assert_eq!(
            text_for(rhs, add).as_deref(),
            Some("Adds one. Twice, really.")
        );
        assert_eq!(crate::protocol::project::tree_violation(&rhs.regions), None);
    }

    #[test]
    fn a_comment_separated_from_the_function_by_code_does_not_count() {
        let after = "// About the constant.\nconst X: u32 = 1;\nfn f() -> u32 {\n    X\n}\n";
        let (file, mut sides) = project("a.rs", "", after);
        Docstrings.apply(&file, &mut sides).unwrap();
        let (Pairing::Both { rhs: rhs_side, .. } | Pairing::RightOnly { rhs: rhs_side }) = &sides
        else {
            panic!("an after side");
        };
        assert!(docstrings(rhs_side).is_empty());
    }

    #[test]
    fn a_python_string_first_in_the_body_is_its_docstring() {
        let after =
            "def f(a):\n    \"\"\"Double a.\n\n    Returns an int.\n    \"\"\"\n    return a * 2\n";
        let (file, mut sides) = project("a.py", "", after);
        Docstrings.apply(&file, &mut sides).unwrap();
        let (Pairing::Both { rhs, .. } | Pairing::RightOnly { rhs }) = &sides else {
            panic!("an after side");
        };
        let f = function_state(rhs, "def f(");
        let docs = docstrings(rhs);
        assert!(!docs.is_empty(), "{docs:?}");
        assert!(
            docs.iter()
                .all(|&(s, e, state)| s >= 1 && e <= 5 && state == f),
            "{docs:?}"
        );
        assert_eq!(
            text_for(rhs, f).as_deref(),
            Some("Double a. Returns an int.")
        );
    }

    #[test]
    fn a_docstring_collapses_with_its_function_and_shares_its_fold_state() {
        let after = "/// Documented.\nfn gone() -> u32 {\n    let a = 1;\n    let b = 2;\n    let c = 3;\n    a + b + c\n}\n";
        let (file, mut sides) = project("a.rs", "", after);
        Docstrings.apply(&file, &mut sides).unwrap();
        let (Pairing::Both { rhs, .. } | Pairing::RightOnly { rhs }) = &mut sides else {
            panic!("the after side exists");
        };
        walk_mut(&mut rhs.regions, &mut |region| {
            if is_function(region) {
                collapse(region, "summary".to_owned());
            }
        });
        DocstringVisibility.apply(&file, &mut sides).unwrap();
        let (Pairing::Both { rhs, .. } | Pairing::RightOnly { rhs }) = &sides else {
            panic!("an after side");
        };
        let state = function_state(rhs, "fn gone(");
        let mut seen = false;
        walk(&rhs.regions, &mut |region| {
            if is_docstring(region) {
                seen = true;
                assert_eq!(region.fold_state_id, state);
                assert!(region.visibility.collapsed);
                assert!(region.visibility.label.is_empty());
            }
        });
        assert!(seen);
    }

    #[test]
    fn a_paired_docstring_over_a_paired_function_bundles_on_both_sides() {
        let before = "/// Stays.\nfn f() -> u32 {\n    1\n}\n";
        let after = "/// Stays.\nfn f() -> u32 {\n    2\n}\n";
        let (file, mut sides) = project("a.rs", before, after);
        Docstrings.apply(&file, &mut sides).unwrap();
        let Pairing::Both { lhs, rhs } = &sides else {
            panic!("both sides");
        };
        let (lhs_docs, rhs_docs) = (docstrings(lhs), docstrings(rhs));
        assert_eq!(lhs_docs.len(), 1, "{lhs_docs:?}");
        assert_eq!(lhs_docs, rhs_docs);
        assert_eq!(lhs_docs[0].2, function_state(lhs, "fn f("));
    }
}
