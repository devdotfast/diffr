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
mod conflicts;
mod constants;
mod diff;
mod display;
mod engine;
mod exit_codes;
use engine::diff_file_content;
mod files;
mod git;
mod gitattributes;
mod hash;
mod line_parser;
mod lines;
mod options;
mod pairing;
mod parse;
pub(crate) mod protocol;
mod summary;
mod version;
mod words;

#[macro_use]
extern crate log;

use crate::config::Params;
use display::style::print_warning;
use log::info;
use options::{FilePermissions, USAGE};

use crate::conflicts::{apply_conflict_markers, START_LHS_MARKER};
use crate::display::style::print_error;
use crate::exit_codes::{EXIT_BAD_ARGUMENTS, EXIT_FOUND_CHANGES, EXIT_SUCCESS};
use crate::files::{
    guess_content, read_file_or_die, read_files_or_die, read_or_die, relative_paths_in_either,
    ProbableFileKind,
};
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

use humansize::{format_size, FormatSizeOptions, BINARY};
use std::path::Path;
use std::thread;

use owo_colors::OwoColorize;
use rayon::prelude::*;
use strum::IntoEnumIterator;
use typed_arena::Arena;

use crate::engine::QueryConflict;
use crate::options::{DiffOptions, DisplayMode, DisplayOptions, FileArgument, Mode};
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
fn main() {
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
        Mode::ListLanguages {
            use_color,
            language_overrides,
        } => {
            for (lang_override, globs) in language_overrides {
                let mut name = match lang_override {
                    LanguageOverride::Language(lang) => language_name(lang),
                    LanguageOverride::PlainText => "Text",
                }
                .to_owned();
                if use_color {
                    name = name.bold().to_string();
                }
                println!("{} (from override)", name);
                for glob in globs {
                    print!(" {}", glob.as_str());
                }
                println!();
            }

            for language in Language::iter() {
                let mut name = language_name(language).to_owned();
                if use_color {
                    name = name.bold().to_string();
                }
                println!("{}", name);

                for glob in language_globs(language) {
                    print!(" {}", glob.as_str());
                }
                println!();
            }
        }
        Mode::DiffFromConflicts {
            display_path,
            path,
            diff_options,
            display_options,
            set_exit_code,
            language_overrides,
            binary_overrides,
        } => {
            let diff_result = diff_or_die(
                diff_conflicts_file(
                    params,
                    &display_path,
                    &path,
                    &display_options,
                    &diff_options,
                    &language_overrides,
                    &binary_overrides,
                ),
                display_options.use_color,
            );

            print_diff_result(&display_options, &diff_result);

            let exit_code = if set_exit_code && diff_result.has_reportable_change() {
                EXIT_FOUND_CHANGES
            } else {
                EXIT_SUCCESS
            };
            std::process::exit(exit_code);
        }
        Mode::Diff {
            diff_options,
            display_options,
            set_exit_code,
            language_overrides,
            binary_overrides,
            lhs_path,
            rhs_path,
            lhs_permissions,
            rhs_permissions,
            display_path,
            renamed,
        } => {
            if lhs_path == rhs_path {
                let is_dir = match &lhs_path {
                    FileArgument::NamedPath(path) => path.is_dir(),
                    _ => false,
                };

                print_warning(
                    &format!(
                        "You've specified the same {} twice.",
                        if is_dir { "directory" } else { "file" }
                    ),
                    &display_options,
                );
            }

            let mut encountered_changes = false;
            match (&lhs_path, &rhs_path) {
                (
                    options::FileArgument::NamedPath(lhs_path),
                    options::FileArgument::NamedPath(rhs_path),
                ) if lhs_path.is_dir() && rhs_path.is_dir() => {
                    // Diffs in parallel when iterating this iterator.
                    let diff_iter = diff_directories(
                        params,
                        lhs_path,
                        rhs_path,
                        &display_options,
                        &diff_options,
                        &language_overrides,
                        &binary_overrides,
                    );

                    if matches!(display_options.display_mode, DisplayMode::Json) {
                        let results: Vec<_> = diff_iter.collect();
                        encountered_changes = results
                            .iter()
                            .any(|diff_result| diff_result.has_reportable_change());
                        display::json::print_directory(results, display_options.print_unchanged);
                    } else if display_options.sort_paths {
                        let mut result: Vec<DiffResult> = diff_iter.collect();
                        result.sort_unstable_by(|a, b| a.display_path.cmp(&b.display_path));
                        for diff_result in result {
                            print_diff_result(&display_options, &diff_result);

                            if diff_result.has_reportable_change() {
                                encountered_changes = true;
                            }
                        }
                    } else {
                        // We want to diff files in the directory in
                        // parallel, but print the results serially
                        // (to prevent display interleaving).
                        // https://github.com/rayon-rs/rayon/issues/210#issuecomment-551319338
                        thread::scope(|s| {
                            let (send, recv) = std::sync::mpsc::sync_channel(1);

                            // I don't see a nice way of fixing this, and difftastic is regularly benchmarked
                            // for performance issues and this hasn't been a problem.
                            #[allow(clippy::result_large_err)]
                            s.spawn(move || {
                                diff_iter
                                    .try_for_each_with(send, |s, diff_result| s.send(diff_result))
                                    .expect("Receiver should be connected")
                            });

                            for diff_result in recv.into_iter() {
                                print_diff_result(&display_options, &diff_result);

                                if diff_result.has_reportable_change() {
                                    encountered_changes = true;
                                }
                            }
                        });
                    }
                }
                _ => {
                    let diff_result = diff_or_die(
                        diff_file(
                            params,
                            &display_path,
                            renamed,
                            &lhs_path,
                            &rhs_path,
                            lhs_permissions.as_ref(),
                            rhs_permissions.as_ref(),
                            &display_options,
                            &diff_options,
                            false,
                            &language_overrides,
                            &binary_overrides,
                        ),
                        display_options.use_color,
                    );
                    if diff_result.has_reportable_change() {
                        encountered_changes = true;
                    }

                    match display_options.display_mode {
                        DisplayMode::Inline
                        | DisplayMode::SideBySide
                        | DisplayMode::SideBySideShowBoth => {
                            print_diff_result(&display_options, &diff_result);
                        }
                        DisplayMode::Json => display::json::print(&diff_result),
                    }
                }
            }

            let exit_code = if set_exit_code && encountered_changes {
                EXIT_FOUND_CHANGES
            } else {
                EXIT_SUCCESS
            };
            std::process::exit(exit_code);
        }
        Mode::GitHasUnmergedFile { display_path } => {
            println!("Unmerged path: {display_path}");
        }
    };
}

/// Print a diff between two files.
fn diff_file(
    params: &Params,
    display_path: &str,
    renamed: Option<String>,
    lhs_path: &FileArgument,
    rhs_path: &FileArgument,
    lhs_permissions: Option<&FilePermissions>,
    rhs_permissions: Option<&FilePermissions>,
    display_options: &DisplayOptions,
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
            let has_byte_changes = if lhs_bytes == rhs_bytes {
                None
            } else {
                Some((lhs_bytes.len(), rhs_bytes.len()))
            };
            return Ok(DiffResult {
                extra_info: renamed,
                display_path: display_path.to_owned(),
                file_format: FileFormat::Binary,
                lhs_src: FileContent::Binary,
                rhs_src: FileContent::Binary,
                lhs_positions: vec![],
                rhs_positions: vec![],
                hunks: vec![],
                lhs_folds: vec![],
                rhs_folds: vec![],
                has_byte_changes,
                has_syntactic_changes: false,
            });
        }
        (ProbableFileKind::Text(lhs_src), ProbableFileKind::Text(rhs_src), _) => (lhs_src, rhs_src),
    };

    if diff_options.strip_cr {
        lhs_src.retain(|c| c != '\r');
        rhs_src.retain(|c| c != '\r');
    }

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

    let mut extra_info = renamed;
    if let (Some(lhs_perms), Some(rhs_perms)) = (lhs_permissions, rhs_permissions) {
        if lhs_perms != rhs_perms {
            let msg = format!(
                "File permissions changed from {} to {}.",
                lhs_perms, rhs_perms
            );

            if let Some(extra_info) = &mut extra_info {
                extra_info.push('\n');
                extra_info.push_str(&msg);
            } else {
                extra_info = Some(msg);
            }
        }
    }

    diff_file_content(
        params,
        display_path,
        extra_info,
        lhs_path,
        rhs_path,
        &lhs_src,
        &rhs_src,
        display_options,
        diff_options,
        overrides,
    )
}

fn diff_conflicts_file(
    params: &Params,
    display_path: &str,
    path: &FileArgument,
    display_options: &DisplayOptions,
    diff_options: &DiffOptions,
    overrides: &[(LanguageOverride, Vec<glob::Pattern>)],
    binary_overrides: &[glob::Pattern],
) -> Result<DiffResult, QueryConflict> {
    let bytes = read_file_or_die(path);
    let mut src = match guess_content(&bytes, path, binary_overrides) {
        ProbableFileKind::Text(src) => src,
        ProbableFileKind::Binary => {
            print_error(
                "Expected a text file with conflict markers, got a binary file.",
                display_options.use_color,
            );
            std::process::exit(EXIT_BAD_ARGUMENTS);
        }
    };

    if diff_options.strip_cr {
        src.retain(|c| c != '\r');
    }

    let conflict_files = match apply_conflict_markers(&src) {
        Ok(cf) => cf,
        Err(msg) => {
            print_error(&msg, display_options.use_color);
            std::process::exit(EXIT_BAD_ARGUMENTS);
        }
    };

    if conflict_files.num_conflicts == 0 {
        print_error(
            &format!(
                "Difftastic requires two paths, or a single file with conflict markers {}.\n",
                if display_options.use_color {
                    START_LHS_MARKER.bold().to_string()
                } else {
                    START_LHS_MARKER.to_owned()
                }
            ),
            display_options.use_color,
        );

        eprintln!("USAGE:\n\n    {}\n", USAGE);
        eprintln!("For more information try --help");
        std::process::exit(EXIT_BAD_ARGUMENTS);
    }

    let lhs_name = match conflict_files.lhs_name {
        Some(name) => format!("'{}'", name),
        None => "the left file".to_owned(),
    };
    let rhs_name = match conflict_files.rhs_name {
        Some(name) => format!("'{}'", name),
        None => "the right file".to_owned(),
    };

    let extra_info = format!(
        "Showing the result of replacing every conflict in {} with {}.",
        lhs_name, rhs_name
    );

    diff_file_content(
        params,
        display_path,
        Some(extra_info),
        path,
        path,
        &conflict_files.lhs_content,
        &conflict_files.rhs_content,
        display_options,
        diff_options,
        overrides,
    )
}

/// Given two directories that contain the files, compare them
/// pairwise. Returns an iterator, so we can print results
/// incrementally.
///
/// When more than one file is modified, the hg extdiff extension passes directory
/// paths with all the modified files.
fn diff_directories<'a>(
    params: &'a Params,
    lhs_dir: &'a Path,
    rhs_dir: &'a Path,
    display_options: &DisplayOptions,
    diff_options: &DiffOptions,
    overrides: &[(LanguageOverride, Vec<glob::Pattern>)],
    binary_overrides: &[glob::Pattern],
) -> impl ParallelIterator<Item = DiffResult> + 'a {
    let diff_options = diff_options.clone();
    let display_options = display_options.clone();
    let overrides: Vec<_> = overrides.into();
    let binary_overrides: Vec<_> = binary_overrides.into();

    // We greedily list all files in the directory, and then diff them
    // in parallel. This is assuming that diffing is slower than
    // enumerating files, so it benefits more from parallelism.
    let paths = relative_paths_in_either(lhs_dir, rhs_dir);

    paths.into_par_iter().map(move |rel_path| {
        info!("Relative path is {:?} inside {:?}", rel_path, lhs_dir);

        let lhs_path = FileArgument::NamedPath(Path::new(lhs_dir).join(&rel_path));
        let rhs_path = FileArgument::NamedPath(Path::new(rhs_dir).join(&rel_path));

        diff_or_die(
            diff_file(
                params,
                &rel_path.display().to_string(),
                None,
                &lhs_path,
                &rhs_path,
                lhs_path.permissions().as_ref(),
                rhs_path.permissions().as_ref(),
                &display_options,
                &diff_options,
                true,
                &overrides,
                &binary_overrides,
            ),
            display_options.use_color,
        )
    })
}

/// The difftastic-style modes stop at a query conflict, as they do at an
/// unreadable file.
fn diff_or_die(result: Result<DiffResult, QueryConflict>, use_color: bool) -> DiffResult {
    match result {
        Ok(diff) => diff,
        Err(conflict) => {
            print_error(&conflict.to_string(), use_color);
            std::process::exit(EXIT_BAD_ARGUMENTS);
        }
    }
}

/// The syntax dumps stop at a fold query conflict.
fn conflict_or_die<T>(result: Result<T, Conflict>) -> T {
    match result {
        Ok(value) => value,
        Err(conflict) => {
            eprintln!(
                "line {}: fold query patterns {} and {} capture the same {} with different fold ranges",
                conflict.line + 1,
                conflict.patterns.0,
                conflict.patterns.1,
                conflict.kind
            );
            std::process::exit(EXIT_BAD_ARGUMENTS);
        }
    }
}

fn print_diff_result(display_options: &DisplayOptions, summary: &DiffResult) {
    match (&summary.lhs_src, &summary.rhs_src) {
        (FileContent::Text(lhs_src), FileContent::Text(rhs_src)) => {
            let hunks = &summary.hunks;

            if !summary.has_syntactic_changes {
                if display_options.print_unchanged {
                    println!(
                        "{}",
                        display::style::header(
                            &summary.display_path,
                            summary.extra_info.as_ref(),
                            1,
                            1,
                            &summary.file_format,
                            display_options
                        )
                    );
                    match summary.file_format {
                        _ if summary.lhs_src == summary.rhs_src => {
                            println!("No changes.\n");
                        }
                        FileFormat::SupportedLanguage(_) => {
                            println!("No syntactic changes.\n");
                        }
                        _ => {
                            println!("No changes.\n");
                        }
                    }
                }
                return;
            }

            if summary.has_syntactic_changes && hunks.is_empty() {
                println!(
                    "{}",
                    display::style::header(
                        &summary.display_path,
                        summary.extra_info.as_ref(),
                        1,
                        1,
                        &summary.file_format,
                        display_options
                    )
                );
                match summary.file_format {
                    FileFormat::SupportedLanguage(_) => {
                        println!("Has syntactic changes.\n");
                    }
                    _ => {
                        println!("Has changes.\n");
                    }
                }

                return;
            }

            match display_options.display_mode {
                DisplayMode::Inline => {
                    display::inline::print(
                        lhs_src,
                        rhs_src,
                        display_options,
                        &summary.lhs_positions,
                        &summary.rhs_positions,
                        hunks,
                        &summary.display_path,
                        &summary.extra_info,
                        &summary.file_format,
                    );
                }
                DisplayMode::SideBySide | DisplayMode::SideBySideShowBoth => {
                    display::side_by_side::print(
                        hunks,
                        display_options,
                        &summary.display_path,
                        summary.extra_info.as_ref(),
                        &summary.file_format,
                        lhs_src,
                        rhs_src,
                        &summary.lhs_positions,
                        &summary.rhs_positions,
                    );
                }
                DisplayMode::Json => unreachable!(),
            }
        }
        (FileContent::Binary, FileContent::Binary) => {
            if display_options.print_unchanged || summary.has_byte_changes.is_some() {
                println!(
                    "{}",
                    display::style::header(
                        &summary.display_path,
                        summary.extra_info.as_ref(),
                        1,
                        1,
                        &FileFormat::Binary,
                        display_options
                    )
                );

                match summary.has_byte_changes {
                    Some((lhs_len, rhs_len)) => {
                        let format_options = FormatSizeOptions::from(BINARY).decimal_places(1);

                        if lhs_len == 0 {
                            // Strictly speaking this is wrong:
                            // previously we may have had an empty but
                            // existent file. In that case, it's a
                            // file modification instead of a file
                            // creation.
                            //
                            // TODO: Fix this pedantic case.
                            println!(
                                "Binary file added ({}).\n",
                                format_size(rhs_len, format_options),
                            )
                        } else if rhs_len == 0 {
                            println!(
                                "Binary file removed ({}).\n",
                                format_size(lhs_len, format_options),
                            )
                        } else {
                            println!(
                                "Binary file modified (old: {}, new: {}).\n",
                                format_size(lhs_len, format_options),
                                format_size(rhs_len, format_options),
                            )
                        }
                    }
                    None => println!("No changes.\n"),
                }
            }
        }
        (FileContent::Text(_), FileContent::Binary)
        | (FileContent::Binary, FileContent::Text(_)) => {
            // We're diffing a binary file against a text file.
            println!(
                "{}",
                display::style::header(
                    &summary.display_path,
                    summary.extra_info.as_ref(),
                    1,
                    1,
                    &FileFormat::Binary,
                    display_options
                )
            );
            println!("Binary contents changed.\n");
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
            None,
            &FileArgument::from_path_argument(OsStr::new("foo.el")),
            &FileArgument::from_path_argument(OsStr::new("foo.el")),
            s,
            s,
            &DisplayOptions::default(),
            &DiffOptions::default(),
            &[],
        )
        .unwrap();

        assert_eq!(res.lhs_positions, vec![]);
        assert_eq!(res.rhs_positions, vec![]);
    }
}
