//! Rules that start things collapsed: whole files by category, and deleted
//! function bodies.
use super::{collapse, ids, is_fold, line_count, walk_mut, FileMutation, FoldMutation};
use crate::category;
use crate::protocol::{FileChange, Pairing, Problem, Source};

/// Generated and test files start hidden behind a placeholder.
pub(crate) struct HiddenCategories {
    pub(crate) generated: bool,
    pub(crate) tests: bool,
}

impl FileMutation for HiddenCategories {
    fn apply(&self, file: &mut FileChange) -> Result<(), Problem> {
        let label = match file.category.as_deref() {
            Some(category::GENERATED) if self.generated => "Generated file · hidden by default",
            Some(category::TEST) if self.tests => "Test file · hidden by default",
            _ => return Ok(()),
        };
        file.visibility.collapsed = true;
        file.visibility.label = label.to_owned();
        Ok(())
    }
}

/// Deleted bodies of at least `min_lines` start collapsed with a line count.
/// The header line stays visible by fold semantics.
pub(crate) struct DeletedBodies {
    pub(crate) min_lines: usize,
}

impl FoldMutation for DeletedBodies {
    fn apply(&self, _file: &FileChange, sides: &mut Pairing<Source>) -> Result<(), Problem> {
        let rhs_ids = sides.rhs().map(|rhs| ids(&rhs.regions)).unwrap_or_default();
        let Some(lhs) = lhs_mut(sides) else {
            return Ok(());
        };
        walk_mut(&mut lhs.regions, &mut |region| {
            if is_fold(region)
                && region.tags.iter().any(|tag| tag == "body")
                && !rhs_ids.contains(&region.id)
                && line_count(region) >= self.min_lines
            {
                let count = line_count(region);
                collapse(region, format!("{count} lines removed"));
            }
        });
        Ok(())
    }
}

fn lhs_mut(sides: &mut Pairing<Source>) -> Option<&mut Source> {
    match sides {
        Pairing::Both { lhs, .. } | Pairing::LeftOnly { lhs } => Some(lhs),
        Pairing::RightOnly { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mutate::walk;
    use crate::protocol::{FileRef, FileStatus, Visibility};

    fn manifest(category: Option<&str>) -> FileChange {
        FileChange {
            file: Pairing::RightOnly {
                rhs: FileRef {
                    path: "x".to_owned(),
                    oid: String::new(),
                    mode: String::new(),
                },
            },
            status: FileStatus::Added,
            category: category.map(str::to_owned),
            language: None,
            visibility: Visibility::default(),
        }
    }

    #[test]
    fn hidden_categories_follow_the_switches() {
        let rule = HiddenCategories {
            generated: true,
            tests: false,
        };
        let mut file = manifest(Some("generated"));
        rule.apply(&mut file).unwrap();
        assert!(file.visibility.collapsed);
        assert_eq!(file.visibility.label, "Generated file · hidden by default");
        let mut file = manifest(Some("test"));
        rule.apply(&mut file).unwrap();
        assert!(!file.visibility.collapsed);
        let mut file = manifest(None);
        rule.apply(&mut file).unwrap();
        assert!(!file.visibility.collapsed);
    }

    #[test]
    fn deleted_bodies_collapse_when_large_and_one_sided() {
        let before = "def gone():\n    a()\n    b()\n    c()\n\ndef kept():\n    a()\n    b()\n    c()\n\ndef tiny():\n    a()\n";
        let after = "def kept():\n    a()\n    b()\n    c()\n";
        let (file, mut sides) = crate::mutate::summarize::tests::project("m.py", before, after);
        DeletedBodies { min_lines: 3 }
            .apply(&file, &mut sides)
            .unwrap();
        let lhs = sides.lhs().unwrap();
        let mut collapsed = Vec::new();
        walk(&lhs.regions, &mut |region| {
            if is_fold(region) && region.visibility.collapsed {
                collapsed.push((region.range.start.line, region.visibility.label.clone()));
            }
        });
        assert_eq!(collapsed, vec![(1, "3 lines removed".to_owned())]);
    }
}
