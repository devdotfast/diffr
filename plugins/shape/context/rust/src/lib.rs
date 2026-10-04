//! Keep changes and useful context; fold each remaining interval once.
use diffr_plugin_sdk::prelude::*;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::ops::Range;

pub struct Context {
    options: Options,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    lines: u32,
}
fn side(side: Side) -> usize {
    match side {
        Side::Lhs => 0,
        Side::Rhs => 1,
    }
}
fn lines(region: &Region) -> Range<usize> {
    region.range.start.line as usize..region.range.end.line as usize
}
impl Guest for Context {
    type Plugin = Self;
}
impl GuestPlugin for Context {
    fn new(options: String) -> Result<Self, String> {
        Ok(Self {
            options: serde_json::from_str(&options).map_err(|e| format!("invalid options: {e}"))?,
        })
    }

    async fn visit(&self, cursor: &Cursor, phase: Visit) -> Result<bool, String> {
        if phase != Visit::File {
            return Ok(false);
        }
        let regions = cursor.regions();
        let mut kept = [Vec::new(), Vec::new()];
        let mut pairs: BTreeMap<u32, Vec<&RegionView>> = BTreeMap::new();
        for region in &regions {
            let rows = &mut kept[side(region.side)];
            rows.resize(rows.len().max(lines(&region.data).end), false);
            if let Kind::Leaf(leaf) = &region.data.kind {
                pairs.entry(leaf.alignment_id).or_default().push(region);
            }
        }
        for region in &regions {
            let data = &region.data;
            let range = lines(data);
            let rows = &mut kept[side(region.side)];
            // Summaries supplied by other plugins are visible landmarks.
            if !data.visibility.label.is_empty() {
                rows[range.clone()].fill(true);
            }
            if !cursor.has_changes(data.id)? {
                continue;
            }
            if matches!(data.kind, Kind::Leaf(_)) {
                let padding = self.options.lines as usize;
                let end = range.end.saturating_add(padding).min(rows.len());
                rows[range.start.saturating_sub(padding)..end].fill(true);
            }
            // Whole-line projection can merge an inline scope into its body.
            // Only a scope that still includes its header supplies context.
            if data.tags.iter().any(|tag| tag == "context:scope")
                && !data.tags.iter().any(|tag| tag == "context:body")
            {
                rows[range.start] = true;
                rows[range.end - 1] = true;
                if let Some(body) = regions.iter().find(|r| {
                    r.data.parent == Some(data.id)
                        && r.data.tags.iter().any(|tag| tag == "context:body")
                }) {
                    rows[range.start..lines(&body.data).start].fill(true);
                }
            }
        }
        // Carry insertion context to the old side through aligned lines.
        for pair in pairs.values().filter(|pair| pair.len() == 2) {
            let a = lines(&pair[0].data);
            let b = lines(&pair[1].data);
            if a.len() == b.len() {
                for (a, b) in a.zip(b) {
                    let visible = kept[0][a] || kept[1][b];
                    kept[0][a] = visible;
                    kept[1][b] = visible;
                }
            } else if kept[0][a.clone()].contains(&true) || kept[1][b.clone()].contains(&true) {
                kept[0][a].fill(true);
                kept[1][b].fill(true);
            }
        }
        // A visible partner prevents collapsing a shared state.
        let mut states = BTreeMap::new();
        for region in &regions {
            let hidden = !kept[side(region.side)][lines(&region.data)].contains(&true);
            let entry = states.entry(region.data.fold_state_id).or_insert(true);
            // An inline callback's scope can project to exactly its body.
            // The explicit body tag wins: its header is already outside.
            let tagged = |name| region.data.tags.iter().any(|tag| tag == name);
            *entry &= hidden && (!tagged("context:scope") || tagged("context:body"));
        }
        for region in &regions {
            if matches!(region.data.kind, Kind::Fold) && states[&region.data.fold_state_id] {
                cursor.set_collapsed(region.data.id, true)?;
            }
        }
        let mut gaps = Vec::new();
        for own in [Side::Lhs, Side::Rhs] {
            let mut breaks = BTreeSet::new();
            let mut previous = None;
            for region in regions.iter().filter(|r| r.side == own) {
                let Kind::Leaf(leaf) = &region.data.kind else {
                    continue;
                };
                let Some(peer) = pairs[&leaf.alignment_id].iter().find(|r| r.side != own) else {
                    continue;
                };
                let other = lines(&peer.data);
                // With zero padding an opposite insertion has no visible line
                // here. It still separates the hidden intervals on this side.
                if let Some(end) = previous {
                    if end > other.start || kept[1 - side(own)][end..other.start].contains(&true) {
                        breaks.insert(lines(&region.data).start);
                    }
                }
                previous = Some(other.end);
            }
            let rows = &kept[side(own)];
            let mut start = None;
            for line in 0..=rows.len() {
                let visible = line == rows.len() || rows[line];
                if visible || breaks.contains(&line) {
                    if let Some(start) = start.take() {
                        gaps.push((own, start, line));
                    }
                }
                if !visible {
                    start.get_or_insert(line);
                }
            }
        }
        // All decisions precede edits: wrapping may dissolve crossing ancestors.
        let mut wrapped = BTreeMap::new();
        for (own, start, end) in gaps {
            let id = cursor.wrap_range(own, start as u32, end as u32)?;
            cursor.set_collapsed(id, true)?;
            cursor.set_label(id, Some(&format!("{} unchanged lines", end - start)))?;
            let mut alignments = Vec::new();
            for leaf in cursor.leaves(own, start as u32, end as u32) {
                if let Kind::Leaf(leaf) = cursor.get(leaf)?.data.kind {
                    alignments.push(leaf.alignment_id);
                }
            }
            if let Some(peer) = wrapped.remove(&alignments) {
                cursor.link(&[peer, id])?;
            } else {
                wrapped.insert(alignments, id);
            }
        }
        Ok(false)
    }
}
export_shape!(Context);
