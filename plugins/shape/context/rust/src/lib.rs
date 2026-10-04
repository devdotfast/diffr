//! Keep changes and the context around them; fold every other stretch into one row.
//!
//! Each sibling list is shaped once, when the walk enters it: leaves are cut
//! where kept lines start and stop, and every run of hidden siblings becomes
//! one collapsed fold. An unchanged fold is a unit: it is hidden whole, even
//! where a context line would reach into it.
use diffr_plugin_sdk::prelude::*;
use serde::Deserialize;
use std::collections::BTreeSet;

/// A hidden stretch shorter than this stays open: its row would save no line.
const MIN_GAP: u32 = 2;
pub struct Context {
    options: Options,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    lines: u32,
}
/// A gap's label, from each region's own length.
fn gap_label(cursor: &Cursor) -> impl Fn(u32) -> Result<String, String> + '_ {
    |region| {
        let lines = line_count(&cursor.get(region)?.data);
        Ok(format!("{lines} unchanged lines"))
    }
}
fn line_count(region: &Region) -> u32 {
    region.range.end.line - region.range.start.line
}

impl Guest for Context {
    type Plugin = Self;
}

impl GuestPlugin for Context {
    fn new(options: String) -> Result<Self, String> {
        let options: Options =
            serde_json::from_str(&options).map_err(|e| format!("invalid options: {e}"))?;
        Ok(Self { options })
    }

    async fn visit(&self, cursor: &Cursor, phase: Visit) -> Result<bool, String> {
        if phase == Visit::Post {
            return Ok(true);
        }
        let id = cursor.id();
        let data = cursor.get(id)?.data;
        if data.visibility.collapsed {
            return Ok(false);
        }
        if data.parent.is_none() {
            // The top level has no parent to shape it, so its first open node
            // does. Shaping is idempotent: a repeat finds every run folded.
            let siblings = cursor.siblings(id)?;
            let mut first = true;
            for &earlier in siblings.iter().take_while(|&&s| s != id) {
                first &= cursor.get(earlier)?.data.visibility.collapsed;
            }
            if first {
                self.shape(cursor, &siblings)?;
            }
        }
        let RegionView { data, children, .. } = cursor.get(id)?;
        if data.visibility.collapsed || cursor.ancestors(id)?.iter().any(|r| r.visibility.collapsed)
        {
            return Ok(false);
        }
        match data.kind {
            Kind::Leaf(_) => Ok(false),
            Kind::Fold => {
                if self.hidden(cursor, id)? {
                    return Ok(false);
                }
                self.shape(cursor, &children)?;
                Ok(true)
            }
        }
    }
}

impl Context {
    /// Cut this list's leaves at kept/hidden edges, then fold each hidden run.
    fn shape(&self, cursor: &Cursor, ids: &[u32]) -> Result<(), String> {
        let mut items = Vec::new();
        for &id in ids {
            items.extend(self.pieces(cursor, id)?);
        }
        let mut run = Vec::new();
        for (id, hidden) in items {
            if hidden {
                run.push(id);
            } else {
                self.fold_run(cursor, &std::mem::take(&mut run))?;
            }
        }
        self.fold_run(cursor, &run)
    }

    /// One item per wholly kept or wholly hidden piece of `id`, in order.
    fn pieces(&self, cursor: &Cursor, id: u32) -> Result<Vec<(u32, bool)>, String> {
        let RegionView { side, data, .. } = cursor.get(id)?;
        if data.visibility.collapsed || matches!(data.kind, Kind::Fold) {
            return Ok(vec![(id, self.hidden(cursor, id)?)]);
        }
        let kept = self.kept(cursor, id)?;
        let len = line_count(&data);
        let mut edges = vec![0];
        for line in 1..len {
            let start = data.range.start.line;
            if kept.contains(&(start + line)) != kept.contains(&(start + line - 1)) {
                edges.push(line);
            }
        }
        let hidden = |offset: u32| !kept.contains(&(data.range.start.line + offset));
        let mut pieces = Vec::new();
        for &offset in edges[1..].iter().rev() {
            pieces.push((cut(cursor, id, offset, side)?, hidden(offset)));
        }
        pieces.push((id, hidden(0)));
        pieces.reverse();
        Ok(pieces)
    }

    /// Fold a run of hidden siblings into one collapsed row, with its peers.
    fn fold_run(&self, cursor: &Cursor, run: &[u32]) -> Result<(), String> {
        let (Some(&first), Some(&last)) = (run.first(), run.last()) else {
            return Ok(());
        };
        let start = cursor.get(first)?.data;
        let lines = cursor.get(last)?.data.range.end.line - start.range.start.line;
        if lines < MIN_GAP || (run.len() == 1 && start.visibility.collapsed) {
            return Ok(());
        }
        if run.len() == 1 {
            return collapse(cursor, first, gap_label(cursor));
        }
        for &member in run {
            self.close(cursor, member)?;
        }
        let gap = self.wrap(cursor, run)?;
        collapse(cursor, gap, gap_label(cursor))
    }

    /// Wrap consecutive hidden siblings in one fold, with their peers.
    fn wrap(&self, cursor: &Cursor, run: &[u32]) -> Result<u32, String> {
        let mut ids = run.to_vec();
        // A peer run that is not hidden too, or does not line up, is shaped
        // when the walk reaches the other side.
        if let Some(peers) = cursor.matching_siblings(run)? {
            let mut hidden = true;
            for &peer in &peers {
                hidden &= self.hidden(cursor, peer)?;
            }
            if hidden {
                ids.extend(peers);
            }
        }
        Ok(match cursor.join(&ids)? {
            RegionIds::Both((lhs, _)) | RegionIds::LeftOnly(lhs) => lhs,
            RegionIds::RightOnly(rhs) => rhs,
        })
    }

    /// How a fold inside a row starts, so that opening the row reads as an
    /// outline. A comment stays open: it names the code below it. A scope
    /// keeps its header and closer, with its body closed as one fold, or
    /// open when it is too short for a row. A scope with no body, or any
    /// other fold, closes whole. A fold holding a region another plugin
    /// collapsed stays open, so that summary shows once the row opens.
    fn close(&self, cursor: &Cursor, id: u32) -> Result<(), String> {
        let RegionView { data, children, .. } = cursor.get(id)?;
        let tagged = |region: &Region, name: &str| region.tags.iter().any(|tag| tag == name);
        if matches!(data.kind, Kind::Leaf(_))
            || data.visibility.collapsed
            || tagged(&data, "context:comment")
            || cursor.display(id)?.collapsed > 0
        {
            return Ok(());
        }
        if tagged(&data, "context:scope") {
            // The body is the scope's one `context:body` child. A list that
            // holds several holds nested scopes' bodies; without exactly one,
            // the body is every line between the scope's first and last.
            let mut bodies = Vec::new();
            for &child in &children {
                if tagged(&cursor.get(child)?.data, "context:body") {
                    bodies.push(child);
                }
            }
            let body = match bodies[..] {
                [body] => Some(vec![body]),
                _ => self.inner(cursor, id)?,
            };
            if let Some(body) = body {
                let (Some(&first), Some(&last)) = (body.first(), body.last()) else {
                    return Ok(());
                };
                let start = cursor.get(first)?.data.range.start.line;
                if cursor.get(last)?.data.range.end.line - start < MIN_GAP {
                    return Ok(());
                }
                let body = match body.len() {
                    1 => first,
                    _ => self.wrap(cursor, &body)?,
                };
                return collapse(cursor, body, |region| {
                    Ok(format!("{} lines", line_count(&cursor.get(region)?.data)))
                });
            }
        }
        collapse(cursor, id, |region| {
            let text = cursor.text(region)?;
            let head = text
                .lines()
                .next()
                .ok_or(format!("region {region} covers no line"))?
                .trim();
            Ok(format!("{head} … {} lines", text.lines().count()))
        })
    }

    /// The children between a scope's first line and its last, cutting the
    /// leaves that hold those lines. None when either line is in a fold.
    fn inner(&self, cursor: &Cursor, id: u32) -> Result<Option<Vec<u32>>, String> {
        let RegionView {
            side,
            data,
            children,
        } = cursor.get(id)?;
        let (Some(&first), Some(&last)) = (children.first(), children.last()) else {
            return Ok(None);
        };
        for end in [first, last] {
            if matches!(cursor.get(end)?.data.kind, Kind::Fold) {
                return Ok(None);
            }
        }
        if line_count(&data) < 3 {
            return Ok(Some(Vec::new()));
        }
        // The closer first: `first` keeps its id when it is also `last`.
        let closer = line_count(&cursor.get(last)?.data);
        if closer > 1 {
            cut(cursor, last, closer - 1, side)?;
        }
        if line_count(&cursor.get(first)?.data) > 1 {
            cut(cursor, first, 1, side)?;
        }
        let children = cursor.get(id)?.children;
        Ok(Some(children[1..children.len() - 1].to_vec()))
    }

    /// Unchanged on both sides, with no kept line.
    ///
    /// An unchanged fold is a unit only where the other side holds its
    /// partner: both hide whole, so rows stay aligned. A fold with no partner
    /// hides only when none of its lines is kept, like the lines across from it.
    fn hidden(&self, cursor: &Cursor, id: u32) -> Result<bool, String> {
        let RegionView { side, data, .. } = cursor.get(id)?;
        if matches!(data.kind, Kind::Leaf(_)) {
            return Ok(self.kept(cursor, id)?.is_empty());
        }
        // A changed scope's header, such as a doc comment above its
        // signature, is kept even where it is an unchanged fold.
        if self.changed(cursor, id)? || !self.signatures(cursor, id)?.is_empty() {
            return Ok(false);
        }
        for region in cursor.linked_regions(id)? {
            if cursor.get(region)?.side != side {
                return Ok(true);
            }
        }
        for leaf in cursor.leaves(side, data.range.start.line, data.range.end.line) {
            if !self.kept(cursor, leaf)?.is_empty() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Changed bytes or unpaired leaves under this region or a linked one.
    fn changed(&self, cursor: &Cursor, id: u32) -> Result<bool, String> {
        for region in cursor.linked_regions(id)? {
            if cursor.has_changes(region)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Kept lines of a leaf, its own and those its paired leaf keeps. An
    /// unpaired leaf is a change: every line is kept.
    fn kept(&self, cursor: &Cursor, id: u32) -> Result<BTreeSet<u32>, String> {
        let data = cursor.get(id)?.data;
        let lines = data.range.start.line..data.range.end.line;
        let Some(peer) = cursor.paired_leaf(id)? else {
            return Ok(lines.collect());
        };
        let other = cursor.get(peer)?.data;
        let mut kept = self.visible(cursor, id)?;
        kept.extend(
            self.visible(cursor, peer)?
                .into_iter()
                .map(|line| lines.start + line - other.range.start.line),
        );
        Ok(kept)
    }

    /// Visible rows of this node: nearby changed rows and its scope headers.
    fn visible(&self, cursor: &Cursor, id: u32) -> Result<BTreeSet<u32>, String> {
        let RegionView { side, data, .. } = cursor.get(id)?;
        let lines = data.range.start.line..data.range.end.line;
        let mut kept = BTreeSet::new();
        let nearby = cursor.leaves(
            side,
            lines.start.saturating_sub(self.options.lines),
            lines.end.saturating_add(self.options.lines),
        );
        for leaf in nearby {
            let peer = cursor.paired_leaf(leaf)?;
            let changed = cursor.has_changes(leaf)?
                || match peer {
                    Some(peer) => cursor.has_changes(peer)?,
                    None => true,
                };
            if changed {
                let data = cursor.get(leaf)?.data;
                let range = data.range.start.line..data.range.end.line;
                kept.extend(
                    range
                        .start
                        .saturating_sub(self.options.lines)
                        .max(lines.start)
                        ..range.end.saturating_add(self.options.lines).min(lines.end),
                );
            }
        }
        kept.extend(self.headers(cursor, id)?);
        Ok(kept)
    }

    /// Lines of this node that a changed enclosing scope keeps: its first
    /// and last line and, with a body fold, everything above the body.
    fn headers(&self, cursor: &Cursor, id: u32) -> Result<BTreeSet<u32>, String> {
        let mut kept = self.signatures(cursor, id)?;
        let data = cursor.get(id)?.data;
        let lines = data.range.start.line..data.range.end.line;
        for scope in self.changed_scopes(cursor, id)? {
            let last = scope.range.end.line - 1;
            if lines.contains(&last) {
                kept.insert(last);
            }
        }
        Ok(kept)
    }

    /// The header lines of this node's changed enclosing scopes: each
    /// scope's first line and, with a body fold, everything above the body.
    /// They win over hiding an unchanged fold whole; a scope's last line
    /// does not, since in Python it is the end of whatever comes last.
    fn signatures(&self, cursor: &Cursor, id: u32) -> Result<BTreeSet<u32>, String> {
        let data = cursor.get(id)?.data;
        let lines = data.range.start.line..data.range.end.line;
        let mut kept = BTreeSet::new();
        for scope in self.changed_scopes(cursor, id)? {
            let start = scope.range.start.line;
            if lines.contains(&start) {
                kept.insert(start);
            }
            for child in cursor.get(scope.id)?.children {
                let data = cursor.get(child)?.data;
                if data.tags.iter().any(|tag| tag == "context:body") {
                    kept.extend(start.max(lines.start)..data.range.start.line.min(lines.end));
                    break;
                }
            }
        }
        Ok(kept)
    }

    /// The enclosing `context:scope` folds that changed on either side.
    fn changed_scopes(&self, cursor: &Cursor, id: u32) -> Result<Vec<Region>, String> {
        let mut scopes = Vec::new();
        for scope in cursor.ancestors(id)? {
            if scope.tags.iter().any(|tag| tag == "context:scope")
                && self.changed(cursor, scope.id)?
            {
                scopes.push(scope);
            }
        }
        Ok(scopes)
    }
}

/// Cut a leaf and its partner; the new tail on the leaf's own side.
fn cut(cursor: &Cursor, id: u32, offset: u32, side: Side) -> Result<u32, String> {
    match (cursor.cut(id, offset)?, side) {
        (RegionIds::Both((piece, _)) | RegionIds::LeftOnly(piece), Side::Lhs)
        | (RegionIds::Both((_, piece)) | RegionIds::RightOnly(piece), Side::Rhs) => Ok(piece),
        _ => Err(format!("cutting region {id} left no piece on its side")),
    }
}

/// Collapse a region's shared state and label each linked region.
fn collapse(
    cursor: &Cursor,
    id: u32,
    label: impl Fn(u32) -> Result<String, String>,
) -> Result<(), String> {
    cursor.set_collapsed(id, true)?;
    for region in cursor.linked_regions(id)? {
        cursor.set_label(region, Some(&label(region)?))?;
    }
    Ok(())
}

export_shape!(Context);
