//! CLI option parsing.

use std::env;
use std::ffi::OsStr;
use std::fmt::Display;
use std::path::{Path, PathBuf};

use clap::{crate_authors, crate_description, Arg, ArgAction, Command};

use crate::exit_codes::EXIT_BAD_ARGUMENTS;
use crate::parse::guess_language::{language_override_from_name, LanguageOverride};
use crate::version::VERSION;

pub(crate) const DEFAULT_BYTE_LIMIT: usize = 1_000_000;
// Chosen experimentally: this is sufficiently many for all the sample
// files (the highest is slow_1.rs/slow_2.rs at 1.3M nodes), but
// small enough to terminate in ~5 seconds like the test file in #306.
pub(crate) const DEFAULT_GRAPH_LIMIT: usize = 3_000_000;
pub(crate) const DEFAULT_PARSE_ERROR_LIMIT: usize = 0;

pub(crate) const USAGE: &str = concat!(env!("CARGO_BIN_NAME"), " debug [OPTIONS]");

pub(crate) const DEFAULT_TERMINAL_WIDTH: usize = 80;

#[derive(Debug, Clone)]
pub(crate) struct DiffOptions {
    pub(crate) graph_limit: usize,
    pub(crate) byte_limit: usize,
    pub(crate) parse_error_limit: usize,
    pub(crate) ignore_comments: bool,
    /// The file is tagged `generated`: diff it by line without parsing.
    pub(crate) generated: bool,
}

impl Default for DiffOptions {
    fn default() -> Self {
        Self {
            graph_limit: DEFAULT_GRAPH_LIMIT,
            byte_limit: DEFAULT_BYTE_LIMIT,
            parse_error_limit: DEFAULT_PARSE_ERROR_LIMIT,
            ignore_comments: false,
            generated: false,
        }
    }
}

fn app() -> clap::Command {
    Command::new("Difftastic")
        // Show options in alphabetical order, rather than in
        // declaration order.
        .next_display_order(None)
        .override_usage(USAGE)
        .version(env!("CARGO_PKG_VERSION"))
        .long_version(VERSION.as_str())
        .about(crate_description!())
        .author(crate_authors!())
        .arg(
            Arg::new("dump-syntax")
                .long("dump-syntax")
                .value_name("PATH")
                .action(ArgAction::Set)
                .long_help(
                    "Parse a single file with tree-sitter and display the difftastic syntax tree.",
                ).help_heading("DEBUG OPTIONS"),
        )
        .arg(
            Arg::new("dump-syntax-dot")
                .long("dump-syntax-dot")
                .value_name("PATH")
                .action(ArgAction::Set)
                .long_help(
                    "Parse a single file with tree-sitter and display the difftastic syntax tree, as a DOT graph.",
                ).help_heading("DEBUG OPTIONS"),
        )
        .arg(
            Arg::new("dump-ts")
                .long("dump-ts")
                                .value_name("PATH")
                .action(ArgAction::Set)
                .long_help(
                    "Parse a single file with tree-sitter and display the tree-sitter parse tree.",
                ).help_heading("DEBUG OPTIONS"),
        )
        .arg(
            Arg::new("ignore-comments").long("ignore-comments")
                .action(ArgAction::SetTrue)
                .env("DFT_IGNORE_COMMENTS")
                .help("Don't consider comments when diffing.")
        )
        .arg(
            Arg::new("override").long("override")
                .value_name("GLOB:NAME")
                .action(ArgAction::Append)
                .help(concat!("Associate this glob pattern with this language, overriding normal language detection. For example:

$ ", env!("CARGO_BIN_NAME"), " debug --override='*.c:C++' --dump-syntax file.c

See --list-languages for the list of language names. Language names are matched case insensitively. Overrides may also specify the language \"text\" to treat a file as plain text.

This argument may be given more than once. For example:

$ ", env!("CARGO_BIN_NAME"), " debug --override='CustomFile:json' --override='*.c:text' --dump-syntax file.c

To configure multiple overrides using environment variables, difftastic also accepts DFT_OVERRIDE_1 up to DFT_OVERRIDE_9.

$ export DFT_OVERRIDE='CustomFile:json'
$ export DFT_OVERRIDE_1='*.c:text'
$ export DFT_OVERRIDE_2='*.js:javascript jsx'

When multiple overrides are specified, the first matching override wins."))
                .env("DFT_OVERRIDE")
        )
        .arg(
            Arg::new("list-languages").long("list-languages")
                .action(ArgAction::SetTrue)
                .help("Print all the languages supported by difftastic, along with their recognised extensions.")
        )
        .arg_required_else_help(true)
}

#[derive(Eq, PartialEq, Debug)]
pub(crate) enum FileArgument {
    NamedPath(std::path::PathBuf),
    DevNull,
}

fn try_canonicalize(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.into())
}

fn relative_to_current(path: &Path) -> PathBuf {
    if let Ok(current_path) = std::env::current_dir() {
        let path = try_canonicalize(path);
        let current_path = try_canonicalize(&current_path);

        if let Ok(rel_path) = path.strip_prefix(current_path) {
            return rel_path.into();
        }
    }

    path.into()
}

impl FileArgument {
    /// Return a `FileArgument` that always represents a path that
    /// exists, with the exception of `/dev/null`, which is turned into [FileArgument::DevNull].
    pub(crate) fn from_path_argument(arg: &OsStr) -> Self {
        // For new and deleted files, Git passes `/dev/null` as the reference file.
        if arg == "/dev/null" {
            Self::DevNull
        } else {
            Self::NamedPath(PathBuf::from(arg))
        }
    }
}

impl Display for FileArgument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NamedPath(path) => {
                write!(f, "{}", relative_to_current(path).display())
            }
            Self::DevNull => write!(f, "/dev/null"),
        }
    }
}

pub(crate) enum Mode {
    ListLanguages {
        language_overrides: Vec<(LanguageOverride, Vec<glob::Pattern>)>,
    },
    DumpTreeSitter {
        path: String,
        language_overrides: Vec<(LanguageOverride, Vec<glob::Pattern>)>,
    },
    DumpSyntax {
        path: String,
        ignore_comments: bool,
        language_overrides: Vec<(LanguageOverride, Vec<glob::Pattern>)>,
    },
    DumpSyntaxDot {
        path: String,
        ignore_comments: bool,
        language_overrides: Vec<(LanguageOverride, Vec<glob::Pattern>)>,
    },
}

fn parse_overrides_or_die(raw_overrides: &[String]) -> Vec<(LanguageOverride, Vec<glob::Pattern>)> {
    let mut overrides: Vec<(LanguageOverride, Vec<glob::Pattern>)> = vec![];
    let mut invalid_syntax = false;

    for raw_override in raw_overrides {
        if let Some((glob_str, lang_name)) = raw_override.rsplit_once(':') {
            match glob::Pattern::new(glob_str) {
                Ok(pattern) => {
                    if let Some(language_override) = language_override_from_name(lang_name) {
                        overrides.push((language_override, vec![pattern]));
                    } else {
                        eprintln!("No such language '{}'", lang_name);
                        eprintln!("See --list-languages for the names of all languages available. Language overrides are case insensitive.");
                        invalid_syntax = true;
                    }
                }
                Err(e) => {
                    eprintln!("Invalid glob syntax '{}'", glob_str);
                    eprintln!("Glob parsing error: {}", e.msg);
                    invalid_syntax = true;
                }
            }
        } else {
            eprintln!("Invalid language override syntax '{}'", raw_override);
            eprintln!("Language overrides are in the format 'GLOB:LANG_NAME', e.g. '*.js:JSON'.");
            invalid_syntax = true;
        }
    }

    if invalid_syntax {
        std::process::exit(EXIT_BAD_ARGUMENTS);
    }

    let mut combined_overrides: Vec<(LanguageOverride, Vec<glob::Pattern>)> = vec![];
    for (lang, globs) in overrides {
        if let Some((prev_lang, prev_globs)) = combined_overrides.last_mut() {
            if *prev_lang == lang {
                prev_globs.extend(globs);
            } else {
                combined_overrides.push((lang, globs));
            }
        } else {
            combined_overrides.push((lang, globs));
        }
    }

    combined_overrides
}

/// Parse CLI arguments passed to the binary.
pub(crate) fn parse_args() -> Mode {
    let matches = app().get_matches_from(std::env::args_os().skip(1));

    let ignore_comments = matches.get_flag("ignore-comments");

    let mut raw_overrides: Vec<String> = vec![];
    if let Some(overrides) = matches.get_many("override") {
        raw_overrides = overrides.cloned().collect();
    }
    for i in 1..=9 {
        if let Ok(value) = env::var(format!("DFT_OVERRIDE_{}", i)) {
            raw_overrides.push(value);
        }
    }

    let language_overrides = parse_overrides_or_die(&raw_overrides);

    if matches.get_flag("list-languages") {
        return Mode::ListLanguages { language_overrides };
    }

    if let Some(path) = matches.get_one::<String>("dump-syntax") {
        return Mode::DumpSyntax {
            path: path.to_owned(),
            ignore_comments,
            language_overrides,
        };
    }

    if let Some(path) = matches.get_one::<String>("dump-syntax-dot") {
        return Mode::DumpSyntaxDot {
            path: path.to_owned(),
            ignore_comments,
            language_overrides,
        };
    }

    if let Some(path) = matches.get_one::<String>("dump-ts") {
        return Mode::DumpTreeSitter {
            path: path.to_owned(),
            language_overrides,
        };
    }

    eprintln!("Pass one of --dump-syntax, --dump-syntax-dot, --dump-ts or --list-languages.");
    std::process::exit(EXIT_BAD_ARGUMENTS);
}

/// Try to work out the width of the terminal we're on, or fall back
/// to a sensible default value.
pub(crate) fn detect_terminal_width() -> usize {
    if let Some((terminal_size::Width(columns), _)) = terminal_size::terminal_size() {
        if columns > 0 {
            return columns.into();
        }
    }

    // If we couldn't detect the terminal width, use the
    // shell variable COLUMNS if it's set. This helps with terminals like eshell.
    //
    // https://github.com/Wilfred/difftastic/issues/707
    // https://stackoverflow.com/a/48016366
    if let Ok(columns_env_val) = std::env::var("COLUMNS") {
        if let Ok(columns) = columns_env_val.parse::<usize>() {
            if columns > 0 {
                return columns;
            }
        }
    }

    DEFAULT_TERMINAL_WIDTH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app() {
        app().debug_assert();
    }

    #[test]
    fn test_detect_display_width() {
        // Basic smoke test.
        assert!(detect_terminal_width() > 10);
    }
}
