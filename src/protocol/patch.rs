//! `--format patch`: the shaped file records as a plain-text patch.
//!
//! The rows are the terminal UI's unified rows (`tui/.../diffr/rows.ts`),
//! built from the same region trees: leaves are zipped on `alignment_id`,
//! unchanged lines print as context, and each region that starts collapsed
//! prints as one marker line instead of its lines. Every code line carries
//! its base and head line numbers in the gutter Review's numbered patches
//! use, so a reader can cite a line without counting from a hunk header:
//!
//! ```text
//! diff --git a/src/lib.rs b/src/lib.rs
//!  3  3  impl Parser {
//! @@ … 13 unchanged lines · impl Parser › fn keep(&self) @@
//! 17 17      }
//! 18 18
//! 19 19      fn parse(&self) {
//! 20    -        self.old();
//!    20 +        self.new();
//! ```
//!
//! A summary prints under its marker, one `~` line per line. See
//! `docs/cli.md` for the whole format.
use super::{
    Diff, Event, FileChange, FileRef, FileStatus, Node, Outcome, Region, Source, Visibility,
};
use crate::hash::{DftHashMap, DftHashSet};
use crate::pairing::Pairing;
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{self, Write};

/// The fold the context plugin's queries put around a whole construct, from
/// its signature line to the line that closes it: the breadcrumb's source.
const SCOPE: &str = "context:scope";

/// The tag the summarizer puts on each fold it summarizes; the summary
/// arrives as the fold's label.
const SUMMARIZED: &str = "summarize:pending";

/// Scope headers kept in a breadcrumb, innermost last.
const CRUMBS: usize = 3;

/// Characters kept of one scope header in a breadcrumb.
const CRUMB_WIDTH: usize = 60;

/// Writes file records as patches in manifest order, whatever order the
/// workers finish them in, so the output is deterministic under `-j`.
pub(crate) struct Writer {
    /// Whether regions and files that start collapsed print expanded.
    expand: bool,
    manifest: Vec<FileChange>,
    positions: DftHashMap<(Option<String>, Option<String>), usize>,
    ready: BTreeMap<usize, String>,
    next: usize,
}

impl Writer {
    pub(crate) fn new(expand: bool) -> Self {
        Self {
            expand,
            manifest: Vec::new(),
            positions: DftHashMap::default(),
            ready: BTreeMap::new(),
            next: 0,
        }
    }

    pub(crate) fn event(&mut self, event: &Event, output: &mut impl Write) -> io::Result<()> {
        match event {
            Event::Start { files, .. } => {
                self.manifest = files.clone();
                self.positions = files
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| (paths(&entry.file), index))
                    .collect();
            }
            Event::File {
                file,
                visibility,
                outcome,
            } => {
                let index = self.positions[&paths(file)];
                let mut text = String::new();
                render(
                    &self.manifest[index],
                    visibility,
                    outcome,
                    self.expand,
                    &mut text,
                );
                self.ready.insert(index, text);
                while let Some(text) = self.ready.remove(&self.next) {
                    output.write_all(text.as_bytes())?;
                    self.next += 1;
                }
                output.flush()?;
            }
            // Annotations are applied before a file is rendered.
            Event::Annotations { .. } => {}
            Event::Complete { aborted, .. } => {
                // A run cut short leaves gaps; print what finished, in order.
                for text in std::mem::take(&mut self.ready).into_values() {
                    output.write_all(text.as_bytes())?;
                }
                output.flush()?;
                if let Some(problem) = aborted {
                    eprintln!(
                        "diffr: stopped early ({}): {}",
                        problem.code, problem.message
                    );
                }
            }
        }
        Ok(())
    }
}

/// A file's identity: its path pair.
fn paths(file: &Pairing<FileRef>) -> (Option<String>, Option<String>) {
    match file {
        Pairing::Both { lhs, rhs } => (Some(lhs.path.clone()), Some(rhs.path.clone())),
        Pairing::LeftOnly { lhs } => (Some(lhs.path.clone()), None),
        Pairing::RightOnly { rhs } => (None, Some(rhs.path.clone())),
    }
}

/// One file's patch: Git's header lines, then its rows.
pub(crate) fn render(
    entry: &FileChange,
    visibility: &Visibility,
    outcome: &Outcome,
    expand: bool,
    out: &mut String,
) {
    header(entry, out);
    let diff = match outcome {
        Outcome::Error { error } => {
            let _ = writeln!(out, "Error ({}): {}", error.code, error.message);
            return;
        }
        Outcome::Diff { diff } => diff,
    };
    if visibility.collapsed && !expand {
        let mut what = format!(
            "file folded: {}",
            or(&visibility.label, "hidden by default")
        );
        if let Diff::Text { stats, .. } = diff {
            let _ = write!(
                what,
                ", +{} −{}",
                stats.textual.added, stats.textual.removed
            );
        }
        marker(&what, &[], out);
        return;
    }
    match diff {
        Diff::Binary { .. } => {
            let (old, new) = match &entry.file {
                Pairing::Both { lhs, rhs } => {
                    (format!("a/{}", lhs.path), format!("b/{}", rhs.path))
                }
                Pairing::LeftOnly { lhs } => (format!("a/{}", lhs.path), "/dev/null".to_owned()),
                Pairing::RightOnly { rhs } => ("/dev/null".to_owned(), format!("b/{}", rhs.path)),
            };
            let _ = writeln!(out, "Binary files {old} and {new} differ");
        }
        Diff::Text {
            sides,
            stats,
            structural_changes,
        } => {
            // A file without a grammar is always diffed by line; only a
            // structural diff that gave up is worth a note.
            if let Some(fallback) = stats
                .fallback
                .as_ref()
                .filter(|fallback| fallback.code != "unsupported_language")
            {
                let _ = writeln!(out, "Line diff ({}): {}", fallback.code, fallback.message);
            }
            if !structural_changes.base.is_empty() || !structural_changes.head.is_empty() {
                Rows::new(sides, expand).render(out);
            }
        }
    }
}

fn or<'a>(text: &'a str, default: &'a str) -> &'a str {
    if text.is_empty() {
        default
    } else {
        text
    }
}

/// `diff --git` and the extended header lines Git prints for the change.
/// `index`, `---` and `+++` repeat what this says, as in Review's numbered
/// patches, and are left out.
fn header(entry: &FileChange, out: &mut String) {
    let (old, new) = match &entry.file {
        Pairing::Both { lhs, rhs } => (lhs, rhs),
        Pairing::LeftOnly { lhs } => (lhs, lhs),
        Pairing::RightOnly { rhs } => (rhs, rhs),
    };
    let _ = writeln!(out, "diff --git a/{} b/{}", old.path, new.path);
    match &entry.file {
        Pairing::LeftOnly { lhs } if !lhs.mode.is_empty() => {
            let _ = writeln!(out, "deleted file mode {}", lhs.mode);
        }
        Pairing::RightOnly { rhs } if !rhs.mode.is_empty() => {
            let _ = writeln!(out, "new file mode {}", rhs.mode);
        }
        Pairing::Both { lhs, rhs } if lhs.mode != rhs.mode => {
            let _ = writeln!(out, "old mode {}\nnew mode {}", lhs.mode, rhs.mode);
        }
        _ => {}
    }
    let verb = match entry.status {
        FileStatus::Renamed => "rename",
        FileStatus::Copied => "copy",
        _ => return,
    };
    let _ = writeln!(out, "{verb} from {}\n{verb} to {}", old.path, new.path);
}

/// A region that starts collapsed prints as this one line, naming the
/// innermost scopes around it; a summary, when the region has one, prints
/// under it.
fn marker(text: &str, crumbs: &[String], out: &mut String) {
    let crumbs = &crumbs[crumbs.len().saturating_sub(CRUMBS)..];
    let _ = if crumbs.is_empty() {
        writeln!(out, "@@ … {text} @@")
    } else {
        writeln!(out, "@@ … {text} · {} @@", crumbs.join(" › "))
    };
}

type Side = usize;

/// A `context:scope` fold's first and last line.
#[derive(Clone, Copy)]
struct Scope {
    first: u32,
    last: u32,
}

struct Leaf<'a> {
    alignment: u32,
    state: u32,
    side: Side,
    start: u32,
    end: u32,
    changed: DftHashSet<u32>,
    region: &'a Region,
    /// The scopes around the leaf, outermost first.
    scopes: Vec<Scope>,
}

struct Fold<'a> {
    state: u32,
    side: Side,
    start: u32,
    /// Inclusive; below `start` when the fold hides nothing.
    last_hidden: i64,
    region: &'a Region,
    /// The scopes around the fold and the fold itself when it is one,
    /// outermost first.
    scopes: Vec<Scope>,
}

/// The source lines of `text`, split on `\n` only.
fn source_lines(text: &str) -> Vec<&str> {
    if text.is_empty() {
        return Vec::new();
    }
    text.strip_suffix('\n')
        .unwrap_or(text)
        .split('\n')
        .collect()
}

fn flatten<'a>(
    source: &'a Source,
    lines: &[&str],
    side: Side,
    leaves: &mut Vec<Leaf<'a>>,
    folds: &mut Vec<Fold<'a>>,
) {
    fn visit<'a>(
        region: &'a Region,
        lines: &[&str],
        side: Side,
        scopes: &[Scope],
        leaves: &mut Vec<Leaf<'a>>,
        folds: &mut Vec<Fold<'a>>,
    ) {
        match &region.node {
            Node::Leaf {
                alignment_id,
                changed,
            } => {
                let range = region.range.lines();
                leaves.push(Leaf {
                    alignment: *alignment_id,
                    state: region.fold_state_id,
                    side,
                    start: range.start,
                    end: range.end,
                    changed: changed.iter().map(|span| span.line).collect(),
                    region,
                    scopes: scopes.to_vec(),
                });
            }
            Node::Fold { children } => {
                let (start, end) = (region.range.start, region.range.end);
                // As in the terminal UI: an end at column zero does not
                // touch its line, and an end line hides only when the range
                // reaches the end of its text.
                let hide_end = end.column > 0
                    && lines
                        .get(end.line as usize)
                        .is_some_and(|text| text.trim_end().len() <= end.column as usize);
                let mut scopes = scopes.to_vec();
                if region.tags.iter().any(|tag| tag == SCOPE) {
                    let lines = region.range.lines();
                    scopes.push(Scope {
                        first: lines.start,
                        last: lines.end.saturating_sub(1).max(lines.start),
                    });
                }
                folds.push(Fold {
                    state: region.fold_state_id,
                    side,
                    start: start.line,
                    last_hidden: i64::from(end.line) - i64::from(!hide_end),
                    region,
                    scopes: scopes.clone(),
                });
                for child in children {
                    visit(child, lines, side, &scopes, leaves, folds);
                }
            }
        }
    }
    for region in &source.regions {
        visit(region, lines, side, &[], leaves, folds);
    }
}

/// One file's rows, built the way the terminal UI's unified layout builds
/// them.
struct Rows<'a> {
    texts: [Vec<&'a str>; 2],
    leaves: [Vec<Leaf<'a>>; 2],
    collapsed: DftHashSet<u32>,
    hidden: [Vec<bool>; 2],
    /// The collapsed fold each side shows a marker for, by its first line.
    bands: [DftHashMap<u32, usize>; 2],
    folds: [Vec<Fold<'a>>; 2],
    width: usize,
}

impl<'a> Rows<'a> {
    fn new(sides: &'a Pairing<Source>, expand: bool) -> Self {
        let (lhs, rhs) = match sides {
            Pairing::Both { lhs, rhs } => (Some(lhs), Some(rhs)),
            Pairing::LeftOnly { lhs } => (Some(lhs), None),
            Pairing::RightOnly { rhs } => (None, Some(rhs)),
        };
        let texts = [lhs, rhs].map(|source| source.map_or(Vec::new(), |s| source_lines(&s.text)));
        let mut leaves = [Vec::new(), Vec::new()];
        let mut folds = [Vec::new(), Vec::new()];
        for (side, source) in [lhs, rhs].into_iter().enumerate() {
            if let Some(source) = source {
                flatten(
                    source,
                    &texts[side],
                    side,
                    &mut leaves[side],
                    &mut folds[side],
                );
            }
        }
        // Regions sharing a fold state open and close together, on either
        // side.
        let mut collapsed = DftHashSet::default();
        if !expand {
            for fold in folds.iter().flatten() {
                if fold.region.visibility.collapsed {
                    collapsed.insert(fold.state);
                }
            }
            for leaf in leaves.iter().flatten() {
                if leaf.region.visibility.collapsed {
                    collapsed.insert(leaf.state);
                }
            }
        }
        let mut hidden = [vec![false; texts[0].len()], vec![false; texts[1].len()]];
        let mut bands = [DftHashMap::default(), DftHashMap::default()];
        for side in 0..2 {
            let mut shown: Vec<usize> = (0..folds[side].len())
                .filter(|&index| collapsed.contains(&folds[side][index].state))
                .collect();
            for &index in &shown {
                let fold = &folds[side][index];
                for line in i64::from(fold.start)..=fold.last_hidden {
                    if let Some(slot) = hidden[side].get_mut(line as usize) {
                        *slot = true;
                    }
                }
            }
            // Outermost first: a fold starting inside one already taken is
            // nested in it and never seen.
            shown.sort_by_key(|&index| {
                let fold = &folds[side][index];
                (fold.start, std::cmp::Reverse(fold.last_hidden))
            });
            let mut covered = -1;
            for index in shown {
                let fold = &folds[side][index];
                if i64::from(fold.start) <= covered {
                    continue;
                }
                bands[side].insert(fold.start, index);
                covered = fold.last_hidden;
            }
        }
        let width = texts[0].len().max(texts[1].len()).max(1).to_string().len();
        Self {
            texts,
            leaves,
            collapsed,
            hidden,
            bands,
            folds,
            width,
        }
    }

    fn render(&self, out: &mut String) {
        let mut rows = Emitter {
            out,
            old: Vec::new(),
            new: Vec::new(),
            gap: None,
        };
        // Zip: walk the left leaves; a partner ahead on the right flushes
        // what precedes it as right-only. Paired leaves come in the same
        // order on both sides.
        let partners: DftHashMap<u32, usize> = self.leaves[1]
            .iter()
            .enumerate()
            .map(|(index, leaf)| (leaf.alignment, index))
            .collect();
        let mut cursor = 0;
        for left in &self.leaves[0] {
            let Some(&partner) = partners.get(&left.alignment) else {
                self.leaf_rows(Some(left), None, &mut rows);
                continue;
            };
            for right in &self.leaves[1][cursor.min(partner)..partner] {
                self.leaf_rows(None, Some(right), &mut rows);
            }
            self.leaf_rows(Some(left), Some(&self.leaves[1][partner]), &mut rows);
            cursor = partner + 1;
        }
        for right in &self.leaves[1][cursor.min(self.leaves[1].len())..] {
            self.leaf_rows(None, Some(right), &mut rows);
        }
        rows.flush();
    }

    fn leaf_rows(&self, left: Option<&Leaf<'_>>, right: Option<&Leaf<'_>>, rows: &mut Emitter<'_>) {
        let anchor = left.or(right).expect("a leaf on one side");
        // Leaves are split at fold edges, so a collapsed fold's marker goes
        // before the leaf it would have started with.
        let band = |leaf: Option<&Leaf<'_>>| {
            let leaf = leaf?;
            let index = *self.bands[leaf.side].get(&leaf.start)?;
            Some(&self.folds[leaf.side][index])
        };
        match (band(left), band(right)) {
            (Some(lhs), Some(rhs))
                if lhs.state == rhs.state
                    || lhs.region.visibility.label == rhs.region.visibility.label =>
            {
                self.fold_marker(rhs, rows)
            }
            (lhs, rhs) => {
                for fold in [lhs, rhs].into_iter().flatten() {
                    self.fold_marker(fold, rows);
                }
            }
        }
        if self.collapsed.contains(&anchor.state) {
            if !self.is_hidden(anchor.side, anchor.start) {
                // The head side's label, as the terminal UI shows it.
                let leaf = right.unwrap_or(anchor);
                self.region_marker(
                    leaf.region,
                    leaf.side,
                    leaf.start,
                    leaf.end - leaf.start,
                    &leaf.scopes,
                    rows,
                );
            }
            return;
        }
        let length = |leaf: Option<&Leaf<'_>>| leaf.map_or(0, |leaf| leaf.end - leaf.start);
        for offset in 0..length(left).max(length(right)) {
            let line = |leaf: Option<&Leaf<'_>>| {
                let leaf = leaf.filter(|leaf| offset < leaf.end - leaf.start)?;
                let line = leaf.start + offset;
                Some((line, self.is_hidden(leaf.side, line)))
            };
            let (l, r) = (line(left), line(right));
            // A paired line is changed when either side paints a change on
            // it, or its text differs anyway; a one-sided line always is.
            let changed = match (l, r) {
                (Some((l, _)), Some((r, _))) => {
                    left.unwrap().changed.contains(&l)
                        || right.unwrap().changed.contains(&r)
                        || self.texts[0][l as usize] != self.texts[1][r as usize]
                }
                (Some((l, _)), None) => right.is_none() || left.unwrap().changed.contains(&l),
                (None, Some((r, _))) => left.is_none() || right.unwrap().changed.contains(&r),
                (None, None) => continue,
            };
            // An unchanged pair folded away on either side is already
            // counted by that side's marker.
            let folded = l.is_some_and(|(_, hidden)| hidden) || r.is_some_and(|(_, hidden)| hidden);
            if folded && !changed {
                continue;
            }
            let shown = |line: Option<(u32, bool)>| {
                line.filter(|&(_, hidden)| !hidden).map(|(line, _)| line)
            };
            let (l, r) = (shown(l), shown(r));
            if !changed {
                let text = match r {
                    Some(r) => self.texts[1][r as usize],
                    None => self.texts[0][l.unwrap() as usize],
                };
                rows.line(self.row(l, r, ' ', text));
                continue;
            }
            if let Some(l) = l {
                rows.removed(self.row(Some(l), None, '-', self.texts[0][l as usize]));
            }
            if let Some(r) = r {
                rows.added(self.row(None, Some(r), '+', self.texts[1][r as usize]));
            }
        }
    }

    fn is_hidden(&self, side: Side, line: u32) -> bool {
        self.hidden[side]
            .get(line as usize)
            .copied()
            .unwrap_or(false)
    }

    /// `<base> <head> <marker><text>`, numbers 1-based and right-aligned to
    /// the file's widest, blank on the side a line is not on.
    fn row(&self, base: Option<u32>, head: Option<u32>, marker: char, text: &str) -> String {
        let number = |line: Option<u32>| match line {
            Some(line) => format!("{:>1$}", line + 1, self.width),
            None => " ".repeat(self.width),
        };
        format!("{} {} {marker}{text}\n", number(base), number(head))
    }

    fn fold_marker(&self, fold: &Fold<'_>, rows: &mut Emitter<'_>) {
        let lines = (fold.last_hidden - i64::from(fold.start) + 1).max(0) as u32;
        self.region_marker(
            fold.region,
            fold.side,
            fold.start,
            lines,
            &fold.scopes,
            rows,
        );
    }

    fn region_marker(
        &self,
        region: &Region,
        side: Side,
        start: u32,
        lines: u32,
        scopes: &[Scope],
        rows: &mut Emitter<'_>,
    ) {
        let label = &region.visibility.label;
        let count = match lines {
            1 => "1 line".to_owned(),
            lines => format!("{lines} lines"),
        };
        // A summary, on a fold the summarizer picked or any multi-line
        // label, prints under the marker rather than in it.
        let summarized = !label.is_empty()
            && (label.contains('\n') || region.tags.iter().any(|tag| tag == SUMMARIZED));
        let text = if summarized {
            format!("{count}, summarized")
        } else if !label.is_empty() {
            label.clone()
        } else if region.tags.iter().any(|tag| tag == SUMMARIZED) {
            // Annotations were skipped, or the summary was discarded.
            format!("{count}, not summarized")
        } else if region.tags.iter().any(|tag| tag.ends_with(":docstring")) {
            format!("{count} of documentation")
        } else {
            count
        };
        let crumbs: Vec<String> = self
            .scope_headers(side, scopes)
            .into_iter()
            .filter_map(crumb)
            .collect();
        if let Some(count) = unchanged(&text).filter(|_| !summarized) {
            rows.gap(count, crumbs);
            return;
        }
        rows.flush();
        marker(&text, &crumbs, rows.out);
        if summarized {
            // Indented like the code it stands in for.
            let first = self.texts[side].get(start as usize).copied().unwrap_or("");
            let indent = &first[..first.len() - first.trim_start().len()];
            for line in label.lines() {
                rows.out
                    .push_str(&self.row(None, None, '~', &format!("{indent}{line}")));
            }
        }
    }

    /// The line that names each scope, outermost first. A construct that
    /// starts after other code on its line (`export class A {`, `app.get(…,
    /// () => {`) has its region start on the next line, inside its body; the
    /// header is then the line above, level with the bracket that closes the
    /// scope. That bracket is on the scope's last line, or the next when the
    /// construct ends mid-line (`});`).
    fn scope_headers(&self, side: Side, scopes: &[Scope]) -> Vec<&str> {
        let lines = &self.texts[side];
        let indent = |line: &str| line.len() - line.trim_start().len();
        let mut headers = Vec::new();
        let mut parent = None;
        for scope in scopes {
            let (start, end) = (
                scope.first as usize,
                (scope.last as usize + 1).min(lines.len()),
            );
            // Attributes, decorators and doc comments the construct starts
            // with name nothing.
            let Some(first) = (start..end)
                .find(|&line| {
                    !PREAMBLE
                        .iter()
                        .any(|preamble| lines[line].trim_start().starts_with(preamble))
                })
                .or((start < end).then_some(start))
            else {
                continue;
            };
            let closing = [end - 1, end]
                .into_iter()
                .filter_map(|line| lines.get(line).copied())
                .find(|line| {
                    line.trim_start().starts_with(['}', ')', ']'])
                        && indent(line) < indent(lines[first])
                });
            let above = start.checked_sub(1);
            let header = match (above, closing) {
                // The line above is the header unless it is the enclosing
                // scope's: then this construct opens that scope's body.
                (Some(above), Some(closing))
                    if indent(lines[above]) == indent(closing)
                        && !lines[above].trim().is_empty()
                        && parent != Some(above) =>
                {
                    above
                }
                _ => first,
            };
            headers.push(lines[header]);
            parent = Some(header);
        }
        headers
    }
}

/// The line count of a context gap's label, `"12 unchanged lines"`.
fn unchanged(label: &str) -> Option<u32> {
    let count = label
        .strip_suffix(" unchanged lines")
        .or_else(|| label.strip_suffix(" unchanged line"))?;
    count.parse().ok()
}

fn unchanged_label(count: u32) -> String {
    match count {
        1 => "1 unchanged line".to_owned(),
        count => format!("{count} unchanged lines"),
    }
}

/// How the lines before a construct's signature start.
const PREAMBLE: [&str; 5] = ["#[", "@", "//", "/*", "* "];

/// Statements the context queries also treat as scopes, which name nothing.
const STATEMENTS: [&str; 8] = [
    "return", "for", "while", "if", "switch", "match", "else", "try",
];

/// A scope's header line as a breadcrumb: trimmed, without the brace or
/// colon that opens its body, and cut short when long. A statement scope
/// (`for`, `return` …) names nothing and is left out.
fn crumb(line: &str) -> Option<String> {
    let text = line.trim();
    let word = text
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .next()
        .unwrap_or("");
    if text.is_empty() || STATEMENTS.contains(&word) {
        return None;
    }
    let text = text
        .strip_suffix('{')
        .or_else(|| text.strip_suffix(':'))
        .unwrap_or(text)
        .trim_end();
    if text.chars().count() <= CRUMB_WIDTH {
        return Some(text.to_owned());
    }
    let cut: String = text.chars().take(CRUMB_WIDTH - 1).collect();
    Some(format!("{}…", cut.trim_end()))
}

/// Collects a run of removed and added lines so a changed block prints its
/// removals before its additions, as Git's patches do, and adds up
/// consecutive context gaps into one marker under the scopes they share:
/// a run of unchanged methods is one gap in their class.
struct Emitter<'a> {
    out: &'a mut String,
    old: Vec<String>,
    new: Vec<String>,
    gap: Option<(u32, Vec<String>)>,
}

impl Emitter<'_> {
    fn removed(&mut self, row: String) {
        self.close_gap();
        self.old.push(row);
    }

    fn added(&mut self, row: String) {
        self.close_gap();
        self.new.push(row);
    }

    fn line(&mut self, row: String) {
        self.flush();
        self.out.push_str(&row);
    }

    fn gap(&mut self, count: u32, crumbs: Vec<String>) {
        self.write_changes();
        match &mut self.gap {
            Some((total, open)) => {
                *total += count;
                let shared = open.iter().zip(&crumbs).take_while(|(a, b)| a == b).count();
                open.truncate(shared);
            }
            None => self.gap = Some((count, crumbs)),
        }
    }

    fn flush(&mut self) {
        self.write_changes();
        self.close_gap();
    }

    fn write_changes(&mut self) {
        for row in self.old.drain(..).chain(self.new.drain(..)) {
            self.out.push_str(&row);
        }
    }

    fn close_gap(&mut self) {
        if let Some((count, crumbs)) = self.gap.take() {
            marker(&unchanged_label(count), &crumbs, self.out);
        }
    }
}
