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
            if lines < MIN_GAP
                || (run.len() == 1
                    && (start.visibility.collapsed || cursor.display(first)?.collapsed > 0))
            {
                continue;
            }
            let mut row = first;
            if run.len() > 1 {
                let mut ids = run;
                if let Some(peers) = cursor.matching_siblings(&ids)? {
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
        let data = cursor.get(id)?.data;
        if matches!(data.kind, Kind::Leaf(_)) {
            return Ok(self.kept(cursor, id)?.is_empty());
        }
        if !self.item(cursor, id)? {
            return Ok(false);
        }
        // A clause of a changed branch statement stays open with its siblings.
        if tagged(&data, "context:clause") {
            let branches = cursor
                .ancestors(id)?
                .into_iter()
                .find(|ancestor| tagged(ancestor, "context:branches"));
            if let Some(branches) = branches {
                if changed(cursor, branches.id)? {
                    return Ok(false);
                }
            }
        }
        for region in cursor.linked_regions(id)? {
            let RegionView { side, data, .. } = cursor.get(region)?;
            let start = data.range.start.line.saturating_sub(self.options.lines);
            let end = data.range.end.line.saturating_add(self.options.lines);
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
        if keep_region(cursor, id)? || changed(cursor, id)? {
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
        let start = cursor.get(id)?.data.range.start.line;
        let radius = self.options.lines;
        let mut kept = BTreeSet::new();
        for region in std::iter::once(id).chain(cursor.paired_leaf(id)?) {
            let RegionView { side, data, .. } = cursor.get(region)?;
            let lines = data.range.start.line..data.range.end.line;
            // Clip to this leaf, then translate the other side's lines into ours.
            let mut keep = |range: std::ops::Range<u32>| {
                kept.extend(
                    (range.start.max(lines.start)..range.end.min(lines.end))
                        .map(|line| line - lines.start + start),
                );
            };
            let parent_kept = match data.parent {
                Some(parent) => keep_region(cursor, parent)?,
                None => false,
            };
            if keep_region(cursor, region)? || parent_kept {
                keep(lines.clone());
                continue;
            }
            let window = lines.start.saturating_sub(radius)..lines.end.saturating_add(radius);
            for leaf in cursor.leaves(side, window.start, window.end) {
                let leaf = cursor.get(leaf)?.data;
                let Kind::Leaf(spans) = leaf.kind else {
                    return Err(format!("leaves() returned the non-leaf {}", leaf.id));
                };
                if cursor.paired_leaf(leaf.id)?.is_none() {
                    keep(
                        leaf.range.start.line.saturating_sub(radius)
                            ..leaf.range.end.line.saturating_add(radius),
                    );
                } else {
                    for span in spans.changed {
                        let end = span.line.saturating_add(radius).saturating_add(1);
                        keep(span.line.saturating_sub(radius)..end);
                    }
                }
            }
            for scope in cursor.ancestors(region)? {
                let is_scope = tagged(&scope, "context:scope");
                let is_branches = tagged(&scope, "context:branches");
                if !(is_scope || is_branches) || !changed(cursor, scope.id)? {
                    continue;
                }
                if is_branches {
                    for child in cursor.get(scope.id)?.children {
                        let body = cursor.get(child)?.data;
                        if tagged(&body, "context:body") {
                            for line in
                                [body.range.start.line.saturating_sub(1), body.range.end.line]
                            {
                                keep(line..line.saturating_add(1));
                            }
                        }
                    }
                }
                if is_scope {
                    keep(scope.range.start.line..scope.range.start.line + 1);
                    if !tagged(&scope, "context:open-ended") {
                        keep(scope.range.end.line - 1..scope.range.end.line);
                    }
                }
            }
        }
        Ok(kept)
    }
}

/// Whether a region always shows: a `context:keep` region, or a header or
/// `context:relevant` region of a changed scope.
fn keep_region(cursor: &Cursor, id: u32) -> Result<bool, String> {
    let data = cursor.get(id)?.data;
    if tagged(&data, "context:keep") {
        return Ok(true);
    }
    let relevant = tagged(&data, "context:relevant");
    for scope in cursor.ancestors(id)? {
        if !tagged(&scope, "context:scope") {
            continue;
        }
        if relevant {
            return changed(cursor, scope.id);
        }
        for child in cursor.get(scope.id)?.children {
            let body = cursor.get(child)?.data;
            if tagged(&body, "context:body") {
                return Ok(
                    data.range.end.line <= body.range.start.line && changed(cursor, scope.id)?
                );
            }
        }
        return Ok(false);
    }
    Ok(false)
}

/// How an unchanged fold looks: an outline. A scope keeps its signature and
/// closer and folds its body; any other fold folds whole. Every fold inside
/// is folded the same way, so opening one shows the next level. A fold
/// holding a region another plugin collapsed stays open, so that summary
/// shows.
fn outline(cursor: &Cursor, id: u32) -> Result<(), String> {
    let data = cursor.get(id)?.data;
    if !data.visibility.collapsed && cursor.display(id)?.collapsed == 0 {
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
        let range = cursor.get(body.unwrap_or(id))?.data.range;
        if range.end.line - range.start.line >= MIN_GAP {
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
    }
    for child in cursor.get(id)?.children {
        if matches!(cursor.get(child)?.data.kind, Kind::Fold) {
            outline(cursor, child)?;
        }
    }
    Ok(())
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
