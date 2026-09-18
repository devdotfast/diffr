//! Wrap runs of adjacent collapsed regions in one collapsed fold.
//!
//! A run is two or more collapsed siblings, with nothing between them or only
//! open leaves spanning at most two lines in all: a blank line and the next
//! function's header, say. It becomes one collapsed fold labelled with how
//! many collapsed regions it wraps and how many lines it spans, as
//! `"3 collapsed regions · 42 lines"`, so the reader sees one row instead of a
//! stack of them. Expanding the group reveals each child's own row. Which
//! plugin collapsed a region, and why, does not matter.
//!
//! An open fold that shows only collapsed regions, with at most two open
//! lines before, between or after them, such as a scope around a collapsed
//! function body and its closing brace, counts as collapsed: it joins a run
//! as the collapsed regions it shows, and its open lines count as separator
//! lines. A run starts on a collapsed row, and a group hides the open lines
//! its last member shows after its last collapsed row.
//!
//! A run is grouped on its side alone when the other side pairs none of its
//! regions. When the other side holds a run that matches it region for
//! region (the same leaf, or a fold sharing the fold state), both runs are
//! grouped in one join, and the two groups open and close together. A
//! run whose regions are paired across sides some other way stays as it
//! is.
use diffr_plugin_sdk::{
    anyhow, export, has_search_highlights, is_fold, line_count, sides_with_other_ids, Draft,
    FileEntry, Move, Node, Pairing, Plugin, Region, Source,
};
use serde::Deserialize;
use std::collections::BTreeSet;

pub struct Group;

/// The plugin has no options.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {}

/// Open lines a run may show between two of its collapsed regions, and that
/// a fold counting as collapsed may show before, between or after its own.
const MAX_SEPARATOR_LINES: usize = 2;

/// A run to group: its siblings, and how many collapsed regions they show.
struct Run<'a> {
    members: Vec<&'a Region>,
    collapsed: usize,
}

impl Plugin for Group {
    type Options = Options;

    fn new(_options: Options) -> anyhow::Result<Self> {
        Ok(Self)
    }

    fn classify(&self, _file: &FileEntry) -> anyhow::Result<Vec<String>> {
        Ok(Vec::new())
    }

    fn mutate(&self, _file: &FileEntry, sides: &Pairing<Source>) -> anyhow::Result<Vec<Move>> {
        let per_side: Vec<_> = sides_with_other_ids(sides)
            .into_iter()
            .map(|(source, other_ids)| (runs(&source.regions), other_ids))
            .collect();
        let mut groups: BTreeSet<BTreeSet<u32>> = BTreeSet::new();
        let mut draft = Draft::new(sides);
        for (side, (runs, other_ids)) in per_side.iter().enumerate() {
            // A one-sided file has no other side, and so no other runs.
            let other_runs: &[Run<'_>] = match per_side.get(1 - side) {
                Some((runs, _)) => runs,
                None => &[],
            };
            for run in runs {
                let mut regions: Vec<u32> = run.members.iter().map(|region| region.id).collect();
                if run.members.iter().any(|region| other_ids.pairs(region)) {
                    let Some(matched) = other_runs
                        .iter()
                        .find(|other| matches(&run.members, &other.members))
                    else {
                        continue;
                    };
                    regions.extend(matched.members.iter().map(|region| region.id));
                }
                if groups.insert(regions.iter().copied().collect()) {
                    draft.group(regions, label(run))?;
                }
            }
        }
        Ok(draft.into_moves())
    }
}

/// Whether two runs on opposite sides pair region for region: the same
/// leaf, or two folds sharing a fold state.
fn matches(run: &[&Region], other: &[&Region]) -> bool {
    run.len() == other.len()
        && run
            .iter()
            .zip(other)
            .all(|(own, theirs)| match (is_fold(own), is_fold(theirs)) {
                (false, false) => own.alignment_id() == theirs.alignment_id(),
                (true, true) => own.fold_state_id == theirs.fold_state_id,
                _ => false,
            })
}

/// What a region shows when it counts as collapsed: how many collapsed
/// regions, and the open lines before the first and after the last.
struct Shape {
    collapsed: usize,
    leading: usize,
    trailing: usize,
}

/// The shape of `region` when it counts as collapsed: it is collapsed, or it
/// is an open fold that shows at least one collapsed region, with at most
/// [`MAX_SEPARATOR_LINES`] open lines before, between or after them.
fn shape(region: &Region) -> Option<Shape> {
    /// Each row `regions` show: `None` for a collapsed region, or an open
    /// leaf's line count.
    fn rows(regions: &[Region], out: &mut Vec<Option<usize>>) {
        for region in regions {
            match (&region.node, region.visibility.collapsed) {
                (_, true) => out.push(None),
                (Node::Leaf { .. }, false) => out.push(Some(line_count(region))),
                (Node::Fold { children }, false) => rows(children, out),
            }
        }
    }
    let children = match (&region.node, region.visibility.collapsed) {
        (_, true) => {
            return Some(Shape {
                collapsed: 1,
                leading: 0,
                trailing: 0,
            })
        }
        (Node::Leaf { .. }, false) => return None,
        (Node::Fold { children }, false) => children,
    };
    let mut shown = Vec::new();
    rows(children, &mut shown);
    // The open lines in each stretch between collapsed rows, and before the
    // first and after the last.
    let mut stretches = vec![0];
    for row in shown {
        match row {
            None => stretches.push(0),
            Some(lines) => *stretches.last_mut().expect("never empty") += lines,
        }
    }
    (stretches.len() > 1 && stretches.iter().all(|lines| *lines <= MAX_SEPARATOR_LINES)).then(
        || Shape {
            collapsed: stretches.len() - 1,
            leading: stretches[0],
            trailing: stretches[stretches.len() - 1],
        },
    )
}

/// The runs of `regions` and of the open folds under them that are not in a
/// run themselves, in document order.
fn runs(regions: &[Region]) -> Vec<Run<'_>> {
    let mut out = Vec::new();
    collapsed_runs(regions, &mut out);
    out
}

fn collapsed_runs<'a>(regions: &'a [Region], out: &mut Vec<Run<'a>>) {
    let mut own = Vec::new();
    let mut run = Run {
        members: Vec::new(),
        collapsed: 0,
    };
    // The open leaves after the run's last member, and the open lines they
    // and that member show after its last collapsed region.
    let mut separator: Vec<&Region> = Vec::new();
    let mut gap = 0;
    for region in regions {
        if has_search_highlights(region) {
            push_run(&mut run, &mut own);
            separator.clear();
            continue;
        }
        match shape(region) {
            // A run starts with a collapsed row: a group never hides open
            // lines above its first one.
            Some(shape) if run.members.is_empty() && shape.leading > 0 => {}
            Some(shape) if run.members.is_empty() || gap + shape.leading <= MAX_SEPARATOR_LINES => {
                run.members.append(&mut separator);
                run.members.push(region);
                run.collapsed += shape.collapsed;
                gap = shape.trailing;
                continue;
            }
            Some(shape) => {
                push_run(&mut run, &mut own);
                separator.clear();
                if shape.leading == 0 {
                    run.members.push(region);
                    run.collapsed += shape.collapsed;
                    gap = shape.trailing;
                }
                continue;
            }
            None if !run.members.is_empty()
                && !is_fold(region)
                && gap + line_count(region) <= MAX_SEPARATOR_LINES =>
            {
                gap += line_count(region);
                separator.push(region);
                continue;
            }
            None => {}
        }
        push_run(&mut run, &mut own);
        separator.clear();
    }
    push_run(&mut run, &mut own);
    let grouped: Vec<u32> = own
        .iter()
        .flat_map(|run| run.members.iter().map(|region| region.id))
        .collect();
    for region in regions {
        // Nothing under a collapsed fold is visible, and a fold in a run is
        // grouped whole, so neither needs grouping inside.
        if let (Node::Fold { children }, false) = (&region.node, region.visibility.collapsed) {
            if !grouped.contains(&region.id) {
                collapsed_runs(children, out);
            }
        }
    }
    out.extend(own);
}

/// Move `run` into `out` if it has two members or more and shows two
/// collapsed regions or more.
fn push_run<'a>(run: &mut Run<'a>, out: &mut Vec<Run<'a>>) {
    let taken = std::mem::replace(
        run,
        Run {
            members: Vec::new(),
            collapsed: 0,
        },
    );
    if taken.members.len() >= 2 && taken.collapsed >= 2 {
        out.push(taken);
    }
}

/// What a group's collapsed row says: `"3 collapsed regions · 42 lines"`.
fn label(run: &Run<'_>) -> String {
    let lines: usize = run.members.iter().copied().map(line_count).sum();
    format!("{} collapsed regions · {lines} lines", run.collapsed)
}

export!("group", Group);
