use super::*;

fn reasons(options: serde_json::Value, status: FileStatus, tags: &[&str]) -> Vec<Move> {
    let sides = tree::Pairing::RightOnly {
        rhs: tree::Source {
            text: String::new(),
            regions: vec![],
        },
    };
    let mut file = manifest("x", &sides, status);
    file.tags = tags.iter().map(|tag| (*tag).to_owned()).collect();
    moves(&bundled("hide-files", options), &file, &wire(sides)).unwrap()
}

fn hide(reason: &str) -> Vec<Move> {
    vec![
        Move::SetCollapsed((types::ROOT, true)),
        Move::SetLabel((types::ROOT, Some(reason.to_owned()))),
    ]
}

#[test]
fn listed_tags_hide_a_file_and_name_the_reason() {
    let rule = || json!({"tags": ["generated", "test"], "deleted": false});
    assert_eq!(
        reasons(rule(), FileStatus::Added, &["test", "generated"]),
        hide("Generated file · hidden by default")
    );
    assert!(reasons(rule(), FileStatus::Added, &["docs"]).is_empty());
    assert!(reasons(rule(), FileStatus::Deleted, &[]).is_empty());
}

#[test]
fn deleted_files_are_hidden_whatever_their_tags() {
    let rule = json!({"tags": ["test"], "deleted": true});
    assert_eq!(
        reasons(rule, FileStatus::Deleted, &["test"]),
        hide("Deleted file · hidden by default")
    );
}
