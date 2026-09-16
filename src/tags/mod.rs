//! File tags: `generated`, `vendored`, `docs`, `test`, and whatever a
//! repository adds.
//!
//! Bundled rules come first and have the lowest precedence: GitHub
//! Linguist's `generated.rb` (ported in [`generated`]), its `vendor.yml` and
//! `documentation.yml` path patterns (vendored under `linguist/` with
//! Linguist's MIT license), and diffr's own test path rules. Git attributes
//! then decide outright: `linguist-generated`, `linguist-vendored` and
//! `linguist-documentation` add or remove their tag whatever the bundled rules
//! said, and `diffr-tags=a,b` adds tags.

mod generated;

pub(crate) use generated::Prefix;

use git2::{AttrCheckFlags, AttrValue, Repository};
use regex::Regex;
use std::collections::BTreeSet;
use std::fmt;
use std::path::Path;
use std::sync::LazyLock;

pub(crate) const GENERATED: &str = "generated";
pub(crate) const VENDORED: &str = "vendored";
pub(crate) const DOCS: &str = "docs";
pub(crate) const TEST: &str = "test";

/// How much of a file the content rules read.
pub(crate) const PREFIX_BYTES: usize = 8 * 1024;

/// Linguist builds one regex from a YAML list of patterns joined with `|`
/// and matches it anywhere in the repository-relative path.
fn linguist_patterns(yaml: &str) -> Regex {
    let patterns: Vec<String> = serde_yaml_ng::from_str(yaml).expect("Linguist YAML parses");
    Regex::new(&patterns.join("|")).expect("Linguist patterns compile")
}

static VENDOR: LazyLock<Regex> =
    LazyLock::new(|| linguist_patterns(include_str!("linguist/vendor.yml")));
static DOCUMENTATION: LazyLock<Regex> =
    LazyLock::new(|| linguist_patterns(include_str!("linguist/documentation.yml")));

/// Directory names, anywhere in the path, that hold tests.
const TEST_DIRS: &[&str] = &["tests", "test", "__tests__", "spec"];

fn is_test(path: &str) -> bool {
    let mut parts = path.split('/').filter(|part| !part.is_empty());
    let name = parts.next_back().unwrap_or_default();
    parts.any(|dir| TEST_DIRS.contains(&dir))
        || name == "conftest.py"
        || name == "tests.rs"
        || name == "test.rs"
        || name.ends_with("_test.go")
        || name.ends_with("_test.py")
        || name.ends_with("_test.rs")
        || name.ends_with("_tests.rs")
        || (name.starts_with("test_") && name.ends_with(".py"))
        || name.contains(".test.")
        || name.contains(".spec.")
}

/// Every bundled rule that looks at the repository-relative path alone.
pub(crate) fn from_path(path: &str) -> BTreeSet<&'static str> {
    let mut tags = BTreeSet::new();
    if generated::by_path(path) {
        tags.insert(GENERATED);
    }
    if VENDOR.is_match(path) {
        tags.insert(VENDORED);
    }
    if DOCUMENTATION.is_match(path) {
        tags.insert(DOCS);
    }
    if is_test(path) {
        tags.insert(TEST);
    }
    tags
}

/// Whether a Linguist content rule could apply to this path, so the start of
/// the file is worth reading.
pub(crate) fn needs_content(path: &str) -> bool {
    generated::needs_content(path)
}

/// Linguist's content rules over the start of the file.
pub(crate) fn generated_by_content(path: &str, prefix: &Prefix<'_>) -> bool {
    generated::by_content(path, prefix)
}

/// A `diffr-tags` attribute that is not a comma-separated list of tags.
#[derive(Debug)]
pub(crate) struct TagError {
    path: String,
    message: String,
}

impl fmt::Display for TagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

impl std::error::Error for TagError {}

/// What git attributes say about one file. `None` leaves the bundled rules
/// in charge of that tag.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Attributes {
    pub(crate) generated: Option<bool>,
    pub(crate) vendored: Option<bool>,
    pub(crate) docs: Option<bool>,
    pub(crate) added: Vec<String>,
}

impl Attributes {
    /// Look up the file's attributes with git's precedence: the repository's
    /// `$GIT_DIR/info/attributes`, then `.gitattributes` files (deeper first,
    /// working tree then index), then the user-wide file
    /// (`core.attributesFile`, default `$XDG_CONFIG_HOME/git/attributes`),
    /// then the system file.
    pub(crate) fn lookup(
        repo: &Repository,
        path: &str,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        // libgit2 marks set and unset attributes by pointer identity, so
        // each value is classified before it is copied anywhere.
        let attr = |name: &str| -> Result<AttrValue<'_>, git2::Error> {
            Ok(AttrValue::from_bytes(repo.get_attr_bytes(
                Path::new(path),
                name,
                AttrCheckFlags::FILE_THEN_INDEX,
            )?))
        };
        Ok(Self::from_values(
            path,
            attr("linguist-generated")?,
            attr("linguist-vendored")?,
            attr("linguist-documentation")?,
            attr("diffr-tags")?,
        )?)
    }

    fn from_values(
        path: &str,
        generated: AttrValue<'_>,
        vendored: AttrValue<'_>,
        docs: AttrValue<'_>,
        tags: AttrValue<'_>,
    ) -> Result<Self, TagError> {
        let error = |message: String| TagError {
            path: path.to_owned(),
            message,
        };
        let added = match tags {
            AttrValue::Unspecified | AttrValue::False => Vec::new(),
            AttrValue::True => {
                return Err(error(
                    "diffr-tags needs a value, such as diffr-tags=fixture,schema".to_owned(),
                ))
            }
            AttrValue::Bytes(_) => {
                return Err(error("diffr-tags is not valid UTF-8".to_owned()));
            }
            AttrValue::String(value) => parse_tags(value).map_err(error)?,
        };
        Ok(Self {
            generated: linguist_flag(generated),
            vendored: linguist_flag(vendored),
            docs: linguist_flag(docs),
            added,
        })
    }

    /// Apply these attributes over the bundled tags, sorted and deduplicated.
    pub(crate) fn resolve(&self, bundled: BTreeSet<&'static str>) -> Vec<String> {
        let mut tags: BTreeSet<String> = bundled.into_iter().map(str::to_owned).collect();
        for (tag, flag) in [
            (GENERATED, self.generated),
            (VENDORED, self.vendored),
            (DOCS, self.docs),
        ] {
            match flag {
                Some(true) => {
                    tags.insert(tag.to_owned());
                }
                Some(false) => {
                    tags.remove(tag);
                }
                None => {}
            }
        }
        tags.extend(self.added.iter().cloned());
        tags.into_iter().collect()
    }
}

/// Linguist's reading of a boolean attribute: unspecified has no opinion,
/// unset or the string `false` is false, and anything else is true.
fn linguist_flag(value: AttrValue<'_>) -> Option<bool> {
    match value {
        AttrValue::Unspecified => None,
        AttrValue::False => Some(false),
        AttrValue::String("false") => Some(false),
        AttrValue::True | AttrValue::String(_) | AttrValue::Bytes(_) => Some(true),
    }
}

/// `a,b`: each tag is lowercase ASCII letters, digits, `-` and `_`, starting
/// with a letter or digit.
fn parse_tags(value: &str) -> Result<Vec<String>, String> {
    value
        .split(',')
        .map(|tag| {
            let valid = tag
                .chars()
                .next()
                .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
                && tag
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
            if valid {
                Ok(tag.to_owned())
            } else {
                Err(format!(
                    "diffr-tags={value}: {tag:?} is not a tag; use lowercase letters, digits, '-' and '_', separated by commas"
                ))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::Repository;
    use std::fs;

    fn tags(path: &str) -> Vec<&'static str> {
        from_path(path).into_iter().collect()
    }

    #[test]
    fn vendored_paths_follow_vendor_yml() {
        for path in [
            "node_modules/left-pad/index.js",
            "vendor/github.com/pkg/errors/errors.go",
            "third_party/zlib/inflate.c",
            "dist/app.js",
            "static/jquery-3.7.1.min.js",
            "deps/uv/src/uv.c",
            "lib/bootstrap.css",
            "configure",
            "web/cache/data.json",
        ] {
            assert!(tags(path).contains(&VENDORED), "{path}");
        }
        for path in ["src/main.rs", "src/vendors.rs", "distribution/a.py"] {
            assert!(!tags(path).contains(&VENDORED), "{path}");
        }
    }

    #[test]
    fn docs_paths_follow_documentation_yml() {
        for path in [
            "docs/cli.md",
            "Doc/index.rst",
            "README.md",
            "sub/readme.txt",
            "CHANGELOG.md",
            "LICENSE",
            "examples/review/viewer/README.md",
            "CITATION.cff",
        ] {
            assert!(tags(path).contains(&DOCS), "{path}");
        }
        for path in ["src/docs/mod.rs", "src/readme_parser.rs", "notes.md"] {
            assert!(!tags(path).contains(&DOCS), "{path}");
        }
    }

    #[test]
    fn test_paths() {
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
            assert_eq!(tags(path), vec![TEST], "{path}");
        }
        for path in [
            "src/main.rs",
            "testing/helpers.rs",
            "src/testament.py",
            "latest.txt",
        ] {
            assert!(!tags(path).contains(&TEST), "{path}");
        }
    }

    #[test]
    fn a_file_carries_every_tag_that_matches() {
        assert_eq!(tags("tests/snapshots/Cargo.lock"), vec![GENERATED, TEST]);
        assert_eq!(
            tags("node_modules/x/package-lock.json"),
            vec![GENERATED, VENDORED]
        );
    }

    fn resolve(
        path: &str,
        generated: AttrValue<'_>,
        tags: AttrValue<'_>,
    ) -> Result<Vec<String>, TagError> {
        Ok(Attributes::from_values(
            path,
            generated,
            AttrValue::Unspecified,
            AttrValue::Unspecified,
            tags,
        )?
        .resolve(from_path(path)))
    }

    #[test]
    fn attributes_beat_bundled_rules() {
        let none = AttrValue::Unspecified;
        assert_eq!(resolve("Cargo.lock", none, none).unwrap(), vec![GENERATED]);
        assert!(resolve("Cargo.lock", AttrValue::False, none)
            .unwrap()
            .is_empty());
        assert!(resolve("Cargo.lock", AttrValue::String("false"), none)
            .unwrap()
            .is_empty());
        assert_eq!(
            resolve("src/schema.rs", AttrValue::True, none).unwrap(),
            vec![GENERATED]
        );
        assert_eq!(
            resolve("src/schema.rs", AttrValue::String("true"), none).unwrap(),
            vec![GENERATED]
        );
        let docs =
            Attributes::from_values("README.md", none, AttrValue::True, AttrValue::False, none)
                .unwrap()
                .resolve(from_path("README.md"));
        assert_eq!(docs, vec![VENDORED]);
    }

    #[test]
    fn diffr_tags_add_tags_and_reject_bad_syntax() {
        let none = AttrValue::Unspecified;
        assert_eq!(
            resolve(
                "tests/a.json",
                none,
                AttrValue::String("schema,fixture_2,schema")
            )
            .unwrap(),
            vec!["fixture_2", "schema", TEST]
        );
        assert!(resolve("a.json", none, AttrValue::False)
            .unwrap()
            .is_empty());
        for bad in [
            AttrValue::True,
            AttrValue::String("a,,b"),
            AttrValue::String("Schema"),
            AttrValue::String("-a"),
        ] {
            let error = resolve("web/a.json", none, bad).unwrap_err().to_string();
            assert!(error.starts_with("web/a.json: diffr-tags"), "{error}");
        }
    }

    /// A repository whose user-wide attributes file is `core.attributesFile`.
    #[test]
    fn the_user_wide_attributes_file_is_honoured_below_the_repository() {
        let dir = tempfile::tempdir().unwrap();
        let repo = Repository::init(dir.path().join("repo")).unwrap();
        let user = dir.path().join("attributes");
        fs::write(
            &user,
            "*.json linguist-generated diffr-tags=user\nCargo.lock -linguist-generated\n",
        )
        .unwrap();
        repo.config()
            .unwrap()
            .set_str("core.attributesFile", user.to_str().unwrap())
            .unwrap();
        fs::write(
            dir.path().join("repo/.gitattributes"),
            "local.json diffr-tags=repo -linguist-generated\n",
        )
        .unwrap();
        let lookup = |path: &str| {
            Attributes::lookup(&repo, path)
                .unwrap()
                .resolve(from_path(path))
        };
        assert_eq!(lookup("web/schema.json"), vec![GENERATED, "user"]);
        assert_eq!(lookup("sub/Cargo.lock"), Vec::<String>::new());
        assert_eq!(lookup("Cargo.toml"), Vec::<String>::new());
        assert_eq!(lookup("local.json"), vec!["repo"]);
    }
}
