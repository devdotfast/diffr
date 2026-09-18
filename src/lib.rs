//! Difftastic is a syntactic diff tool.
//!
//! For usage instructions and advice on contributing, see [the
//! manual](http://difftastic.wilfred.me.uk/).
//!

// I frequently develop difftastic on a newer rustc than the MSRV, so
// these two aren't relevant.
#![allow(renamed_and_removed_lints)]
// This tends to trigger on larger tuples of simple types, and naming
// them would probably be worse for readability.
#![allow(clippy::type_complexity)]
// == "" is often clearer when dealing with strings.
#![allow(clippy::comparison_to_empty)]
// It's common to have pairs foo_lhs and foo_rhs, leading to double
// the number of arguments and triggering this lint.
#![allow(clippy::too_many_arguments)]
// Has false positives on else if chains that sometimes have the same
// body for readability.
#![allow(clippy::if_same_then_else)]
// Good practice in general, but a necessary evil for Syntax. Its Hash
// implementation does not consider the mutable fields, so it is still
// correct.
#![allow(clippy::mutable_key_type)]
// manual_unwrap_or_default was added in Rust 1.79, so earlier versions of
// clippy complain about allowing it.
#![allow(unknown_lints)]
// It's sometimes more readable to explicitly create a vec than to use
// the Default trait.
#![allow(clippy::manual_unwrap_or_default)]
// I find the explicit arithmetic clearer sometimes.
#![allow(clippy::implicit_saturating_sub)]
// It's helpful being super explicit about byte length versus Unicode
// character point length sometimes.
#![allow(clippy::needless_as_bytes)]
// .to_owned() is more explicit on string references.
#![warn(clippy::str_to_string)]
// .to_string() on a String is clearer as .clone().
#![warn(clippy::string_to_string)]
// Debugging features shouldn't be in checked-in code.
#![warn(clippy::todo)]
#![warn(clippy::dbg_macro)]

mod cli;
mod config;
mod constants;
mod diff;
mod engine;
mod exit_codes;
use engine::diff_file_content;
mod files;
mod git;
mod gitattributes;
mod hash;
mod line_layout;
mod line_parser;
mod lines;
mod options;
mod pairing;
mod parse;
mod plugin;
pub(crate) mod protocol;
pub mod search;
mod summary;
mod tags;
mod version;
mod words;

#[macro_use]
extern crate log;

use crate::config::Params;

use crate::exit_codes::EXIT_BAD_ARGUMENTS;
use crate::files::{guess_content, read_files_or_die, read_or_die, ProbableFileKind};
use crate::gitattributes::{check_diff_attr, DiffAttribute};
use crate::parse::guess_language::{
    guess, language_globs, language_name, Language, LanguageOverride,
};
use crate::parse::syntax;

/// The global allocator used by difftastic.
///
/// Diffing allocates a large amount of memory, and both Jemalloc and
/// MiMalloc perform better than the system allocator.
///
/// Some versions of MiMalloc (specifically libmimalloc-sys greater
/// than 0.1.24) handle very large, mostly unused allocations
/// badly. This makes large line-oriented diffs very slow, as
/// discussed in #297.
///
/// MiMalloc is generally faster than Jemalloc, but older versions of
/// MiMalloc don't compile on GCC 15+, so use Jemalloc for now. See
/// #805.
///
/// For reference, Jemalloc uses 10-20% more time (although up to 33%
/// more instructions) when testing on sample files.
#[cfg(not(any(windows, target_os = "illumos", target_os = "freebsd")))]
use tikv_jemallocator::Jemalloc;

#[cfg(not(any(windows, target_os = "illumos", target_os = "freebsd")))]
#[global_allocator]
static GLOBAL: Jemalloc = Jemalloc;

use std::path::Path;

use strum::IntoEnumIterator;
use typed_arena::Arena;

use crate::engine::QueryConflict;
use crate::options::{DiffOptions, FileArgument, Mode};
use crate::parse::folds::Conflict;
use crate::parse::syntax::init_all_info;
use crate::parse::tree_sitter_parser as tsp;
use crate::summary::{DiffResult, FileContent, FileFormat};

extern crate pretty_env_logger;

/// Terminate the process if we get SIGPIPE.
#[cfg(unix)]
fn reset_sigpipe() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(not(unix))]
fn reset_sigpipe() {
    // Do nothing.
}

/// The entrypoint.
pub fn run_cli() {
    pretty_env_logger::try_init_timed_custom_env("DFT_LOG")
        .expect("The logger has not been previously initialized");
    reset_sigpipe();

    let result = match std::env::args_os().nth(1).as_deref() {
        Some(arg) if arg == "debug" => {
            run_debug();
            return;
        }
        _ => cli::run(),
    };
    match result {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    }
}

fn run_debug() {
    let params = &Params::default();

    match options::parse_args() {
        Mode::DumpTreeSitter {
            path,
            language_overrides,
        } => {
            let path = Path::new(&path);
            let bytes = read_or_die(path);
            let src = String::from_utf8_lossy(&bytes).to_string();

            let language = guess(path, &src, &language_overrides);
            match language {
                Some(lang) => {
                    let ts_lang = tsp::from_language(lang);
                    let tree = tsp::to_tree(&src, ts_lang);
                    tsp::print_tree(&src, &tree);
                }
                None => {
                    eprintln!("No tree-sitter parser for file: {:?}", path);
                }
            }
        }
        Mode::DumpSyntax {
            path,
            ignore_comments,
            language_overrides,
        } => {
            let path = Path::new(&path);
            let bytes = read_or_die(path);
            let src = String::from_utf8_lossy(&bytes).to_string();

            let language = guess(path, &src, &language_overrides);
            match language {
                Some(lang) => {
                    let ts_lang = params.language(lang);
                    let arena = Arena::new();
                    let ast = conflict_or_die(tsp::parse(&arena, &src, ts_lang, ignore_comments));
                    init_all_info(&ast, &[]);
                    println!("{:#?}", ast);
                }
                None => {
                    eprintln!("No tree-sitter parser for file: {:?}", path);
                }
            }
        }
        Mode::DumpSyntaxDot {
            path,
            ignore_comments,
            language_overrides,
        } => {
            let path = Path::new(&path);
            let bytes = read_or_die(path);
            let src = String::from_utf8_lossy(&bytes).to_string();

            let language = guess(path, &src, &language_overrides);
            match language {
                Some(lang) => {
                    let ts_lang = params.language(lang);
                    let arena = Arena::new();
                    let ast = conflict_or_die(tsp::parse(&arena, &src, ts_lang, ignore_comments));
                    init_all_info(&ast, &[]);
                    syntax::print_as_dot(&ast);
                }
                None => {
                    eprintln!("No tree-sitter parser for file: {:?}", path);
                }
            }
        }
        Mode::ListLanguages { language_overrides } => {
            for (lang_override, globs) in language_overrides {
                let name = match lang_override {
                    LanguageOverride::Language(lang) => language_name(lang),
                    LanguageOverride::PlainText => "Text",
                };
                println!("{} (from override)", name);
                for glob in globs {
                    print!(" {}", glob.as_str());
                }
                println!();
            }

            for language in Language::iter() {
                println!("{}", language_name(language));

                for glob in language_globs(language) {
                    print!(" {}", glob.as_str());
                }
                println!();
            }
        }
    };
}

/// Diff two files: `--no-index`.
fn diff_file(
    params: &Params,
    display_path: &str,
    lhs_path: &FileArgument,
    rhs_path: &FileArgument,
    diff_options: &DiffOptions,
    missing_as_empty: bool,
    overrides: &[(LanguageOverride, Vec<glob::Pattern>)],
    binary_overrides: &[glob::Pattern],
) -> Result<DiffResult, QueryConflict> {
    let (lhs_bytes, rhs_bytes) = read_files_or_die(lhs_path, rhs_path, missing_as_empty);

    let (mut lhs_src, mut rhs_src) = match (
        guess_content(&lhs_bytes, lhs_path, binary_overrides),
        guess_content(&rhs_bytes, rhs_path, binary_overrides),
        check_diff_attr(Path::new(display_path)),
    ) {
        (ProbableFileKind::Binary, _, _)
        | (_, ProbableFileKind::Binary, _)
        | (_, _, Some(DiffAttribute::AssumeBinary)) => {
            return Ok(DiffResult {
                file_format: FileFormat::Binary,
                lhs_src: FileContent::Binary,
                rhs_src: FileContent::Binary,
                lhs_positions: vec![],
                rhs_positions: vec![],
                lhs_folds: vec![],
                rhs_folds: vec![],
            });
        }
        (ProbableFileKind::Text(lhs_src), ProbableFileKind::Text(rhs_src), _) => (lhs_src, rhs_src),
    };

    // Ensure that lhs_src and rhs_src both have trailing
    // newlines.
    //
    // This is important when textually diffing files that don't have
    // a trailing newline, e.g. "foo\n\bar\n" versus "foo". We want to
    // consider `foo` to be unchanged in this case.
    //
    // Theoretically a tree-sitter parser could change its AST due to
    // the additional trailing newline, but it seems vanishingly
    // unlikely.
    if !lhs_src.is_empty() && !lhs_src.ends_with('\n') {
        lhs_src.push('\n');
    }
    if !rhs_src.is_empty() && !rhs_src.ends_with('\n') {
        rhs_src.push('\n');
    }

    diff_file_content(
        params,
        display_path,
        lhs_path,
        rhs_path,
        &lhs_src,
        &rhs_src,
        diff_options,
        overrides,
    )
}

/// The syntax dumps stop at a fold query conflict.
fn conflict_or_die<T>(result: Result<T, Conflict>) -> T {
    match result {
        Ok(value) => value,
        Err(conflict) => {
            eprintln!(
                "line {}: {} and {} capture the same {} with different fold ranges",
                conflict.line + 1,
                conflict.sources.0,
                conflict.sources.1,
                conflict.kind
            );
            std::process::exit(EXIT_BAD_ARGUMENTS);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    #[test]
    fn test_diff_identical_content() {
        let s = "foo";
        let res = diff_file_content(
            &Params::default(),
            "foo.el",
            &FileArgument::from_path_argument(OsStr::new("foo.el")),
            &FileArgument::from_path_argument(OsStr::new("foo.el")),
            s,
            s,
            &DiffOptions::default(),
            &[],
        )
        .unwrap();

        assert_eq!(res.lhs_positions, vec![]);
        assert_eq!(res.rhs_positions, vec![]);
    }
}
