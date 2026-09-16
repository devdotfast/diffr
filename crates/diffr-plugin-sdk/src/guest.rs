//! The component side of [`crate::export!`]: the `plugin` resource over a
//! [`Plugin`], with the generated guest bindings' records converted to the
//! contract's Rust records and back around each call.
use crate::bindings::diffr::plugin::types as wit;
use crate::bindings::exports::diffr::plugin::guest;
use crate::types;
use crate::Plugin;

/// One instance of the plugin `P`, the component's `plugin` resource.
pub struct Instance<P>(P);

impl<P: Plugin + 'static> guest::GuestPlugin for Instance<P> {
    fn new(options: String) -> Result<guest::Plugin, String> {
        let options: P::Options =
            serde_json::from_str(&options).map_err(|error| format!("invalid options: {error}"))?;
        let plugin = P::new(options).map_err(|error| format!("{error:#}"))?;
        Ok(guest::Plugin::new(Instance(plugin)))
    }

    fn classify(&self, file: wit::FileEntry) -> Result<Vec<String>, String> {
        self.0
            .classify(&file_entry(file))
            .map_err(|error| format!("{error:#}"))
    }

    fn mutate(
        &self,
        file: wit::FileEntry,
        lhs: Option<wit::Source>,
        rhs: Option<wit::Source>,
    ) -> Result<Vec<wit::Move>, String> {
        let lhs = lhs.map(source);
        let rhs = rhs.map(source);
        self.0
            .mutate(&file_entry(file), lhs.as_ref(), rhs.as_ref())
            .map(|moves| moves.into_iter().map(lower).collect())
            .map_err(|error| format!("{error:#}"))
    }
}

fn file_entry(file: wit::FileEntry) -> types::FileEntry {
    types::FileEntry {
        path: file.path,
        old_path: file.old_path,
        status: match file.status {
            wit::FileStatus::Added => types::FileStatus::Added,
            wit::FileStatus::Deleted => types::FileStatus::Deleted,
            wit::FileStatus::Modified => types::FileStatus::Modified,
            wit::FileStatus::Renamed => types::FileStatus::Renamed,
            wit::FileStatus::Copied => types::FileStatus::Copied,
            wit::FileStatus::TypeChanged => types::FileStatus::TypeChanged,
        },
        tags: file.tags,
    }
}

fn source(side: wit::Source) -> types::Source {
    let position = |position: wit::Position| types::Position {
        line: position.line,
        column: position.column,
    };
    types::Source {
        text: side.text,
        regions: side
            .regions
            .into_iter()
            .map(|region| types::Region {
                id: region.id,
                parent: region.parent,
                fold_state_id: region.fold_state_id,
                range: types::Range {
                    start: position(region.range.start),
                    end: position(region.range.end),
                },
                tags: region.tags,
                visibility: types::Visibility {
                    collapsed: region.visibility.collapsed,
                    label: region.visibility.label,
                },
                kind: match region.kind {
                    wit::Kind::Leaf(leaf) => types::Kind::Leaf(types::Leaf {
                        alignment_id: leaf.alignment_id,
                        changed: leaf
                            .changed
                            .into_iter()
                            .map(|span| types::Span {
                                line: span.line,
                                start_column: span.start_column,
                                end_column: span.end_column,
                            })
                            .collect(),
                    }),
                    wit::Kind::Fold => types::Kind::Fold,
                },
            })
            .collect(),
    }
}

fn lower(next: types::Move) -> wit::Move {
    match next {
        types::Move::Cut { region, at } => wit::Move::Cut(wit::Cut { region, at }),
        types::Move::JoinFolds { regions } => wit::Move::JoinFolds(regions),
        types::Move::LinkFoldState { regions } => wit::Move::LinkFoldState(regions),
        types::Move::SetCollapsed { region, collapsed } => {
            wit::Move::SetCollapsed((region, collapsed))
        }
        types::Move::SetLabel { region, label } => wit::Move::SetLabel((region, label)),
        types::Move::SetTags { region, tags } => wit::Move::SetTags((region, tags)),
    }
}
