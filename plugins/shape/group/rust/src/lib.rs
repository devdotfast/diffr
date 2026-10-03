//! Group adjacent collapsed rows, allowing at most two open separator lines.
use diffr_plugin_sdk::prelude::*;
use serde::Deserialize;

pub struct Group;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {}
const MAX_SEPARATOR_LINES: u32 = 2;

impl Guest for Group {
    type Plugin = Self;
}

impl GuestPlugin for Group {
    fn new(options: String) -> Result<Self, String> {
        let _: Options =
            serde_json::from_str(&options).map_err(|e| format!("invalid options: {e}"))?;
        Ok(Self)
    }

    async fn visit(&self, cursor: &Cursor, phase: Visit) -> Result<bool, String> {
        if phase == Visit::Post {
            return Ok(true);
        }
        let NodeView::Region(RegionView { data, .. }) = cursor.get(cursor.id())? else {
            return Ok(true);
        };
        if cursor
            .ancestors(data.id)?
            .iter()
            .any(|r| r.visibility.collapsed)
        {
            return Ok(false);
        }
        if let Some((mut ids, count)) = run(cursor, data.id)? {
            if let Some(peers) = cursor.matching_siblings(&ids)? {
                let same = peers.is_empty()
                    || (!cursor
                        .ancestors(peers[0])?
                        .iter()
                        .any(|r| r.visibility.collapsed)
                        && run(cursor, peers[0])?.is_some_and(|(run, _)| run == peers));
                if same {
                    let mut lines = 0;
                    for &id in &ids {
                        if let NodeView::Region(RegionView { data, .. }) = cursor.get(id)? {
                            lines += (data.range.end.line - data.range.start.line) as usize;
                        }
                    }
                    ids.extend(peers);
                    let label = format!("{count} collapsed regions · {lines} lines");
                    let folds = match cursor.join(&ids)? {
                        RegionIds::Both((lhs, rhs)) => vec![lhs, rhs],
                        RegionIds::LeftOnly(fold) | RegionIds::RightOnly(fold) => vec![fold],
                    };
                    cursor.set_collapsed(folds[0], true)?;
                    for fold in folds {
                        cursor.set_label(fold, Some(&label))?;
                    }
                    return Ok(false);
                }
            }
        }
        Ok(!data.visibility.collapsed)
    }
}

/// Only the sibling run beginning at this node; no traversal or retained file state.
fn run(cursor: &Cursor, id: u32) -> Result<Option<(Vec<u32>, u32)>, String> {
    let first = cursor.display(id)?;
    if first.collapsed == 0 || first.leading > 0 || first.longest_gap > MAX_SEPARATOR_LINES {
        return Ok(None);
    }
    let mut ids = vec![id];
    let mut count = first.collapsed;
    let mut gap = first.trailing;
    let mut separator = Vec::new();
    for next in cursor
        .siblings(id)?
        .into_iter()
        .skip_while(|next| *next != id)
        .skip(1)
    {
        let rows = cursor.display(next)?;
        if rows.collapsed > 0
            && rows.longest_gap <= MAX_SEPARATOR_LINES
            && gap + rows.leading <= MAX_SEPARATOR_LINES
        {
            ids.append(&mut separator);
            ids.push(next);
            count += rows.collapsed;
            gap = rows.trailing;
        } else if matches!(cursor.get(next)?, NodeView::Region(RegionView {data,..}) if matches!(data.kind, Kind::Leaf(_)))
            && gap + rows.leading <= MAX_SEPARATOR_LINES
        {
            gap += rows.leading;
            separator.push(next);
        } else {
            break;
        }
    }
    Ok((ids.len() >= 2 && count >= 2).then_some((ids, count)))
}

export!(Group);
