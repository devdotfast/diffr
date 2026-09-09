//! Each fixture runs the CLI against Git base/head commits and checks its full output.
//! Open examples/review/real/<fixture>/review.snap to read the expected diff.
mod review_support;

use review_support::{Fixture, Repo};

#[test]
fn review_context() {
    let fixture = Fixture::load("01-review-108");
    let actual = fixture.run();
    fixture.assert_expected(&actual);
}

#[test]
fn parser_return() {
    let fixture = Fixture::load("02-review-175");
    let actual = fixture.run();
    fixture.assert_expected(&actual);
}

#[test]
fn new_file() {
    let fixture = Fixture::load("03-review-160");
    let actual = fixture.run();
    fixture.assert_expected(&actual);
}

#[test]
fn python_method() {
    let fixture = Fixture::load("04-flask-6096");
    let actual = fixture.run();
    fixture.assert_expected(&actual);
}

#[test]
fn new_method() {
    let fixture = Fixture::load("05-flask-6133");
    let actual = fixture.run();
    fixture.assert_expected(&actual);
}

#[test]
fn deleted_block() {
    let fixture = Fixture::load("06-requests-6965");
    let actual = fixture.run();
    fixture.assert_expected(&actual);
}

#[test]
fn rust_tail() {
    let fixture = Fixture::load("07-ripgrep-3487");
    let actual = fixture.run();
    fixture.assert_expected(&actual);
}

#[test]
fn rust_test() {
    let fixture = Fixture::load("08-ripgrep-3496");
    let actual = fixture.run();
    fixture.assert_expected(&actual);
}

#[test]
fn go_return() {
    let fixture = Fixture::load("09-cli-11038");
    let actual = fixture.run();
    fixture.assert_expected(&actual);
}

#[test]
fn go_nested() {
    let fixture = Fixture::load("10-go-git-1492");
    let actual = fixture.run();
    fixture.assert_expected(&actual);
}

#[test]
fn typed_parameter() {
    let fixture = Fixture::load("11-flask-5526");
    let actual = fixture.run();
    fixture.assert_expected(&actual);
}

#[test]
fn missing_refs_and_paths_are_errors() {
    let repo = Repo::new();
    let (base, _) = repo.commit("a.py", Some(b"x = 1\n"), None);
    assert!(!repo.review("invalid-ref", &base, "a.py").status.success());
    assert!(!repo.review(&base, &base, "missing.py").status.success());
    assert!(!repo.review(&base, &base, "../a.py").status.success());
}

#[test]
fn deleted_file_and_identical_refs() {
    let repo = Repo::new();
    let (base, _) = repo.commit("a.py", Some("def café():\n    return 1\n".as_bytes()), None);
    let (head, _) = repo.commit("a.py", None, Some(&base));
    let output = repo.review(&base, &head, "a.py");
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("- def café():"));
    assert!(text.contains("-     return 1"));
    assert!(!text.contains(" + "));
    let output = repo.review(&base, &base, "a.py");
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("(no syntactic changes)"));
}
