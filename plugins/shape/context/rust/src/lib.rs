//! Keep changes and the context around them; fold every other stretch into one row.
//!
//! One walk does it. Entering an unchanged fold folds it to an outline. Entering a leaf
//! cuts it where kept lines start and stop. Leaving a fold folds each run
//! of hidden children into one row. The host's root fold makes the top
//! level one more fold.
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
        let id = cursor.id();
        let data = cursor.get(id)?.data;
        if data.visibility.collapsed {
            return Ok(false);
        }
        match (phase, data.kind) {
            // Only changed folds open. An unchanged fold is an outline: near a
            // change it stays on screen, otherwise it joins its neighbours' row.
            (Visit::Pre, Kind::Fold) if self.item(cursor, id)? => {
                outline(cursor, id)?;
                Ok(false)
            }
            (Visit::Pre, Kind::Leaf(_)) => {
                self.trim(cursor, id)?;
                Ok(false)
            }
            (Visit::Post, Kind::Fold) => {
                self.merge_runs(cursor, id)?;
                Ok(true)
            }
            _ => Ok(true),
        }
    }
}

impl Context {
    /// Cut a leaf where kept lines start and stop, so each piece is wholly
    /// kept or wholly hidden.
    fn trim(&self, cursor: &Cursor, id: u32) -> Result<(), String> {
        let range = cursor.get(id)?.data.range;
        let kept = self.kept(cursor, id)?;
        // From the end, so `id` stays the first piece.
        for line in (range.start.line + 1..range.end.line).rev() {
            if kept.contains(&line) != kept.contains(&(line - 1)) {
                cursor.cut(id, line - range.start.line)?;
            }
        }
        Ok(())
    }

    /// Fold each run of hidden children into one collapsed row, joined with
    /// its peers on the other side when they are hidden too.
    fn merge_runs(&self, cursor: &Cursor, parent: u32) -> Result<(), String> {
        let mut runs = vec![Vec::new()];
        for child in cursor.get(parent)?.children {
            if self.hidden(cursor, child)? {
                runs.last_mut().expect("one run at least").push(child);
            } else {
                runs.push(Vec::new());
            }
        }
        for run in runs {
            let (Some(&first), Some(&last)) = (run.first(), run.last()) else {
                continue;
            };
            let start = cursor.get(first)?.data;
            let lines = cursor.get(last)?.data.range.end.line - start.range.start.line;
            if lines < MIN_GAP || (run.len() == 1 && start.visibility.collapsed) {
                continue;
            }
            let mut row = first;
            if run.len() > 1 {
                let mut ids = run.clone();
                if let Some(peers) = cursor.matching_siblings(&run)? {
                    let mut hidden = true;
                    for &peer in &peers {
                        hidden &= self.hidden(cursor, peer)?;
                    }
                    if hidden {
                        ids.extend(peers);
                    }
                }
                row = match cursor.join(&ids)? {
                    RegionIds::Both((lhs, _)) | RegionIds::LeftOnly(lhs) => lhs,
                    RegionIds::RightOnly(rhs) => rhs,
                };
            }
            collapse(cursor, row, |region| {
                let range = cursor.get(region)?.data.range;
                Ok(format!(
                    "{} unchanged lines",
                    range.end.line - range.start.line
                ))
            })?;
        }
        Ok(())
    }

    /// Whether a region hides in its neighbours' row: a leaf with no kept
    /// line, or an item no change lies within `lines` of, on either side.
    fn hidden(&self, cursor: &Cursor, id: u32) -> Result<bool, String> {
        if matches!(cursor.get(id)?.data.kind, Kind::Leaf(_)) {
            return Ok(self.kept(cursor, id)?.is_empty());
        }
        if !self.item(cursor, id)? {
            return Ok(false);
        }
        for region in cursor.linked_regions(id)? {
            let RegionView { side, data, .. } = cursor.get(region)?;
            let start = data.range.start.line.saturating_sub(self.options.lines);
            let end = data.range.end.line + self.options.lines;
            for leaf in cursor.leaves(side, start, end) {
                if changed(cursor, leaf)? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }

    /// Whether a fold stays shut as one item. It is unchanged, and it is not
    /// the body of a changed scope: that body opens with its scope, so a
    /// changed signature shows the start of the body. It also reaches the
    /// other side, so its twin folds with it and rows stay aligned.
    fn item(&self, cursor: &Cursor, id: u32) -> Result<bool, String> {
        let RegionView { side, data, .. } = cursor.get(id)?;
        let Some(parent) = data.parent else {
            return Ok(false);
        };
        if in_header(cursor, id)? || changed(cursor, id)? {
            return Ok(false);
        }
        if tagged(&data, "context:body")
            && tagged(&cursor.get(parent)?.data, "context:scope")
            && changed(cursor, parent)?
        {
            return Ok(false);
        }
        for region in cursor.linked_regions(id)? {
            if cursor.get(region)?.side != side {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The lines of a leaf that stay visible, its own and those its paired
    /// leaf keeps on the other side.
    fn kept(&self, cursor: &Cursor, id: u32) -> Result<BTreeSet<u32>, String> {
        let data = cursor.get(id)?.data;
        let mut kept = self.kept_on_side(cursor, id)?;
        if let Some(peer) = cursor.paired_leaf(id)? {
            let peer_start = cursor.get(peer)?.data.range.start.line;
            let shift = |line: u32| line - peer_start + data.range.start.line;
            kept.extend(self.kept_on_side(cursor, peer)?.into_iter().map(shift));
        }
        Ok(kept)
    }

    /// Kept lines of a leaf from its own side: all of a changed leaf, the
    /// lines within `lines` of a changed leaf, and the first and last line
    /// of every changed scope around it.
    fn kept_on_side(&self, cursor: &Cursor, id: u32) -> Result<BTreeSet<u32>, String> {
        let RegionView { side, data, .. } = cursor.get(id)?;
        let lines = data.range.start.line..data.range.end.line;
        if changed(cursor, id)? || in_header(cursor, id)? {
            return Ok(lines.collect());
        }
        let mut kept = BTreeSet::new();
        let window = lines.start.saturating_sub(self.options.lines)..lines.end + self.options.lines;
        for leaf in cursor.leaves(side, window.start, window.end) {
            if changed(cursor, leaf)? {
                let near = cursor.get(leaf)?.data.range;
                let start = near.start.line.saturating_sub(self.options.lines);
                let end = near.end.line + self.options.lines;
                kept.extend(start.max(lines.start)..end.min(lines.end));
            }
        }
        for scope in cursor.ancestors(id)? {
            if tagged(&scope, "context:scope") && changed(cursor, scope.id)? {
                for line in [scope.range.start.line, scope.range.end.line - 1] {
                    if lines.contains(&line) {
                        kept.insert(line);
                    }
                }
            }
        }
        Ok(kept)
    }
}

/// How an unchanged fold looks: an outline. A scope keeps its signature and
/// closer and folds its body; any other fold folds whole. Every fold inside
/// is folded the same way, so opening one shows the next level. A comment
/// stays open: it names the code below it. A fold holding a region another
/// plugin collapsed stays open, so that summary shows.
fn outline(cursor: &Cursor, id: u32) -> Result<(), String> {
    let data = cursor.get(id)?.data;
    if !data.visibility.collapsed
        && !tagged(&data, "context:comment")
        && cursor.display(id)?.collapsed == 0
    {
        // A scope's body is its `context:body` child, else its last fold child.
        let mut body = None;
        if tagged(&data, "context:scope") {
            for child in cursor.get(id)?.children {
                let data = cursor.get(child)?.data;
                if tagged(&data, "context:body") {
                    body = Some(child);
                    break;
                }
                if matches!(data.kind, Kind::Fold) {
                    body = Some(child);
                }
            }
        }
        match body {
            Some(body) => collapse(cursor, body, |region| {
                let range = cursor.get(region)?.data.range;
                Ok(format!("{} lines", range.end.line - range.start.line))
            })?,
            None => collapse(cursor, id, |region| {
                let text = cursor.text(region)?;
                let head = text.lines().next().unwrap_or_default().trim();
                Ok(format!("{head} … {} lines", text.lines().count()))
            })?,
        }
    }
    for child in cursor.get(id)?.children {
        if matches!(cursor.get(child)?.data.kind, Kind::Fold) {
            outline(cursor, child)?;
        }
    }
    Ok(())
}

/// Whether a region sits above the `context:body` of a changed scope: its
/// signature, attributes or doc comment.
fn in_header(cursor: &Cursor, id: u32) -> Result<bool, String> {
    let Some(parent) = cursor.get(id)?.data.parent else {
        return Ok(false);
    };
    let scope = cursor.get(parent)?;
    if !tagged(&scope.data, "context:scope") || !changed(cursor, parent)? {
        return Ok(false);
    }
    for child in scope.children {
        let data = cursor.get(child)?.data;
        if tagged(&data, "context:body") {
            return Ok(cursor.get(id)?.data.range.end.line <= data.range.start.line);
        }
    }
    Ok(false)
}

/// Changed bytes or unpaired leaves anywhere in this region's fold state,
/// on either side.
fn changed(cursor: &Cursor, id: u32) -> Result<bool, String> {
    for region in cursor.linked_regions(id)? {
        if cursor.has_changes(region)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn tagged(region: &Region, tag: &str) -> bool {
    region.tags.iter().any(|t| t == tag)
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
