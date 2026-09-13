//! File categories: `source`, `test`, `generated`, or `docs`.
//!
//! A `diffr-classify` git attribute wins outright. Otherwise a set
//! `linguist-generated` attribute means generated, and failing both, the
//! path is matched against built-in rules. Anything else is unclassified.

pub(crate) const GENERATED: &str = "generated";
pub(crate) const TEST: &str = "test";
pub(crate) const DOCS: &str = "docs";

/// Directory names, anywhere in the path, that mark everything under them.
const GENERATED_DIRS: &[&str] = &["dist", "build", "vendor", "node_modules", "__generated__"];
const TEST_DIRS: &[&str] = &["tests", "test", "__tests__", "spec"];
const DOCS_DIRS: &[&str] = &["docs"];

/// Exact file names.
const GENERATED_FILES: &[&str] = &[
    "package-lock.json",
    "pnpm-lock.yaml",
    "yarn.lock",
    "Cargo.lock",
    "go.sum",
    "composer.lock",
    "Gemfile.lock",
    "poetry.lock",
    "uv.lock",
    "bun.lock",
    "bun.lockb",
];
const TEST_FILES: &[&str] = &["conftest.py"];

/// The built-in rule for a repository-relative path, if one matches.
pub(crate) fn from_path(path: &str) -> Option<&'static str> {
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    let name = parts.next_back().unwrap_or_default();
    let dirs: Vec<&str> = parts.collect();
    if dirs.iter().any(|dir| GENERATED_DIRS.contains(dir))
        || GENERATED_FILES.contains(&name)
        || name.ends_with(".lock")
        || name.ends_with(".min.js")
        || name.ends_with(".min.css")
        || name.ends_with(".pb.go")
        || name.contains(".generated.")
        || name.contains("_generated.")
    {
        return Some(GENERATED);
    }
    if dirs.iter().any(|dir| TEST_DIRS.contains(dir))
        || TEST_FILES.contains(&name)
        || name.ends_with("_test.go")
        || name.ends_with("_test.py")
        || name == "tests.rs"
        || name == "test.rs"
        || name.ends_with("_test.rs")
        || name.ends_with("_tests.rs")
        || name.starts_with("test_") && name.ends_with(".py")
        || name.contains(".test.")
        || name.contains(".spec.")
    {
        return Some(TEST);
    }
    if dirs.iter().any(|dir| DOCS_DIRS.contains(dir)) || name.ends_with(".md") {
        return Some(DOCS);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_rules_cover_the_common_layouts() {
        for path in [
            "Cargo.lock",
            "web/pnpm-lock.yaml",
            "dist/app.js",
            "src/vendor/lib.c",
            "assets/app.min.js",
            "api/v1/service.pb.go",
            "schema.generated.ts",
            "src/__generated__/types.ts",
            "src/model_generated.rs",
        ] {
            assert_eq!(from_path(path), Some(GENERATED), "{path}");
        }
        for path in [
            "tests/streaming/check.py",
            "src/foo_test.go",
            "src/App.test.tsx",
            "src/App.spec.ts",
            "pkg/test_widgets.py",
            "pkg/conftest.py",
            "src/__tests__/a.js",
            "spec/models/user_spec.rb",
            "src/review/tests.rs",
            "src/parser/test.rs",
            "src/git_test.rs",
            "src/protocol_tests.rs",
        ] {
            assert_eq!(from_path(path), Some(TEST), "{path}");
        }
        for path in ["README.md", "docs/cli.md", "docs/assets/a.png"] {
            assert_eq!(from_path(path), Some(DOCS), "{path}");
        }
        for path in [
            "src/main.rs",
            "testing/helpers.rs",
            "src/testament.py",
            "latest.txt",
        ] {
            assert_eq!(from_path(path), None, "{path}");
        }
    }

    #[test]
    fn generated_wins_over_test_and_docs() {
        assert_eq!(from_path("tests/fixtures/big.lock"), Some(GENERATED));
        assert_eq!(from_path("docs/build/index.html"), Some(GENERATED));
    }
}
