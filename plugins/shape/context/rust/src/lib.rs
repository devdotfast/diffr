//! Collapse unchanged nodes outside nearby changes and enclosing scope markers.
use diffr_plugin_sdk::prelude::*;
use serde::Deserialize;
use std::collections::BTreeSet;

const MIN_GAP: u32 = 3;
pub struct Context {
    options: Options,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    lines: u32,
}
fn label(lines: u32) -> String {
    format!("{lines} unchanged lines")
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
        let NodeView::Region(RegionView { side, data, .. }) = cursor.get(cursor.id())? else {
            return Ok(true);
        };
        if data.visibility.collapsed {
            return Ok(false);
        }
        let lines = data.range.start.line..data.range.end.line;
        let mut kept = self.visible(cursor, data.id)?;
        match data.kind {
            Kind::Fold => {
                if !kept.is_empty() {
                    return Ok(true);
                }
                let peers = cursor.linked_regions(data.id)?;
                for &peer in &peers {
                    if peer != data.id && !self.visible(cursor, peer)?.is_empty() {
                        return Ok(true);
                    }
                }
                if lines.end - lines.start < MIN_GAP {
                    return Ok(true);
                }
                cursor.set_collapsed(data.id, true)?;
                for peer in peers {
                    let NodeView::Region(RegionView { data, .. }) = cursor.get(peer)? else {
                        unreachable!()
                    };
                    cursor.set_label(
                        peer,
                        Some(&label(data.range.end.line - data.range.start.line)),
                    )?;
                }
                Ok(false)
            }
            Kind::Leaf(_) => {
                let Some(peer) = cursor.paired_leaf(data.id)? else {
                    return Ok(false);
                };
                let NodeView::Region(RegionView { data: other, .. }) = cursor.get(peer)? else {
                    unreachable!()
                };
                kept.extend(
                    self.visible(cursor, peer)?
                        .into_iter()
                        .map(|line| lines.start + line - other.range.start.line),
                );
                let mut gaps = Vec::new();
                let mut start = lines.start;
                for line in kept.into_iter().chain(std::iter::once(lines.end)) {
                    if line - start >= MIN_GAP {
                        gaps.push((start - lines.start, line - lines.start));
                    }
                    start = line + 1;
                }
                let len = lines.end - lines.start;
                for (start, end) in gaps.into_iter().rev() {
                    let mut piece = data.id;
                    if start > 0 {
                        piece = match (cursor.cut(data.id, start)?, side) {
                            (
                                RegionIds::Both((piece, _)) | RegionIds::LeftOnly(piece),
                                Side::Lhs,
                            )
                            | (
                                RegionIds::Both((_, piece)) | RegionIds::RightOnly(piece),
                                Side::Rhs,
                            ) => piece,
                            _ => {
                                return Err(format!(
                                    "cutting region {} left no piece on its side",
                                    data.id
                                ))
                            }
                        };
                    }
                    if end < len {
                        cursor.cut(piece, end - start)?;
                    }
                    let label = label(end - start);
                    let paired = cursor.paired_leaf(piece)?;
                    cursor.set_collapsed(piece, true)?;
                    cursor.set_label(piece, Some(&label))?;
                    if let Some(paired) = paired {
                        cursor.set_label(paired, Some(&label))?;
                    }
                }
                Ok(false)
            }
        }
    }
}

impl Context {
    /// Visible rows of this node: nearby changed rows and enclosing scope markers.
    fn visible(&self, cursor: &Cursor, id: u32) -> Result<BTreeSet<u32>, String> {
        let NodeView::Region(RegionView { side, data, .. }) = cursor.get(id)? else {
            unreachable!()
        };
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
                let NodeView::Region(RegionView { data, .. }) = cursor.get(leaf)? else {
                    unreachable!()
                };
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
        for scope in cursor.ancestors(id)? {
            if !scope.tags.iter().any(|tag| tag == "context:scope")
                || !cursor.has_changes(scope.id)?
            {
                continue;
            }
            let span = scope.range.start.line..scope.range.end.line;
            kept.extend(
                [span.start, span.end - 1]
                    .into_iter()
                    .filter(|line| lines.contains(line)),
            );
            if let NodeView::Region(RegionView { children, .. }) = cursor.get(scope.id)? {
                for child in children {
                    let NodeView::Region(RegionView { data, .. }) = cursor.get(child)? else {
                        unreachable!()
                    };
                    if data.tags.iter().any(|tag| tag == "context:body") {
                        kept.extend(
                            span.start.max(lines.start)..data.range.start.line.min(lines.end),
                        );
                        break;
                    }
                }
            }
        }
        Ok(kept)
    }
}

export!(Context);
