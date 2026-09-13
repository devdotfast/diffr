//! Shared source-to-domain diff computation, independent of CLI and transport.
use crate::config::Params;
use crate::constants::Side;
use crate::diff::changes::ChangeMap;
use crate::diff::shortest_path::{mark_syntax, ExceededGraphLimit};
use crate::diff::sliders::fix_all_sliders;
use crate::diff::unchanged;
use crate::display;
use crate::display::context::opposite_positions;
use crate::display::hunks::{matched_pos_to_hunks, merge_adjacent};
use crate::line_folds;
use crate::line_parser;
use crate::lines::MaxLine;
use crate::options::{DiffOptions, DisplayOptions, FileArgument};
use crate::parse::folds;
use crate::parse::guess_language::{guess, language_name, LanguageOverride};
use crate::parse::syntax::{self, init_next_prev};
use crate::parse::tree_sitter_parser as tsp;
use crate::summary::{DiffResult, FileContent, FileFormat};
use humansize::{format_size, FormatSizeOptions, BINARY};
use std::{env, path::Path};
use typed_arena::Arena;

impl DiffResult {
    #[cfg(test)]
    pub(crate) fn from_sources(path: &str, lhs: &str, rhs: &str) -> Self {
        Self::from_sources_with_params(path, lhs, rhs, &Params::default())
    }

    #[cfg(test)]
    pub(crate) fn from_sources_with_params(
        path: &str,
        lhs: &str,
        rhs: &str,
        params: &Params,
    ) -> Self {
        Self::from_sources_with_options(
            path,
            lhs,
            rhs,
            params,
            &DisplayOptions::default(),
            &DiffOptions::default(),
        )
    }

    pub(crate) fn from_sources_with_options(
        path: &str,
        lhs: &str,
        rhs: &str,
        params: &Params,
        display: &DisplayOptions,
        options: &DiffOptions,
    ) -> Self {
        let file = crate::options::FileArgument::NamedPath(path.into());
        diff_file_content(
            params,
            path,
            None,
            &file,
            &file,
            lhs,
            rhs,
            display,
            options,
            &[],
        )
    }
}
fn check_only_text(
    file_format: &FileFormat,
    display_path: &str,
    extra_info: Option<String>,
    lhs_src: &str,
    rhs_src: &str,
) -> DiffResult {
    let has_byte_changes = if lhs_src == rhs_src {
        None
    } else {
        Some((lhs_src.as_bytes().len(), rhs_src.as_bytes().len()))
    };

    DiffResult {
        display_path: display_path.to_owned(),
        extra_info,
        file_format: file_format.clone(),
        lhs_src: FileContent::Text(lhs_src.into()),
        rhs_src: FileContent::Text(rhs_src.into()),
        lhs_positions: vec![],
        rhs_positions: vec![],
        hunks: vec![],
        lhs_folds: vec![],
        rhs_folds: vec![],
        fold_pairs: vec![],
        has_byte_changes,
        has_syntactic_changes: lhs_src != rhs_src,
    }
}

pub(crate) fn diff_file_content(
    params: &Params,
    display_path: &str,
    extra_info: Option<String>,
    _lhs_path: &FileArgument,
    rhs_path: &FileArgument,
    lhs_src: &str,
    rhs_src: &str,
    display_options: &DisplayOptions,
    diff_options: &DiffOptions,
    overrides: &[(LanguageOverride, Vec<glob::Pattern>)],
) -> DiffResult {
    let mut annotations = display::syntax_context::SyntaxAnnotations::default();
    let guess_src = match rhs_path {
        FileArgument::DevNull => &lhs_src,
        _ => &rhs_src,
    };

    let language = guess(Path::new(display_path), guess_src, overrides);
    let lang_config = language.map(|lang| (lang, params.language(lang)));

    if lhs_src == rhs_src {
        let file_format = match language {
            Some(language) => FileFormat::SupportedLanguage(language),
            None => FileFormat::PlainText,
        };

        // If the two files are byte-for-byte identical, return early
        // rather than doing any more work.
        return DiffResult {
            extra_info,
            display_path: display_path.to_owned(),
            file_format,
            lhs_src: FileContent::Text(lhs_src.into()),
            rhs_src: FileContent::Text(rhs_src.into()),
            lhs_positions: vec![],
            rhs_positions: vec![],
            hunks: vec![],
            lhs_folds: vec![],
            rhs_folds: vec![],
            fold_pairs: vec![],
            has_byte_changes: None,
            has_syntactic_changes: false,
        };
    }

    let mut lhs_folds = Vec::new();
    let mut rhs_folds = Vec::new();
    let (file_format, lhs_positions, rhs_positions) = match lang_config {
        None => {
            let file_format = FileFormat::PlainText;
            if diff_options.check_only {
                return check_only_text(&file_format, display_path, extra_info, lhs_src, rhs_src);
            }

            let (lhs_positions, rhs_positions) = line_parser::change_positions(lhs_src, rhs_src);
            (file_format, lhs_positions, rhs_positions)
        }
        Some((language, lang_config)) => {
            let arena = Arena::new();
            match tsp::to_tree_with_limit(diff_options, lang_config.parser, lhs_src, rhs_src) {
                Ok((lhs_tree, rhs_tree)) => {
                    match tsp::to_syntax_with_limit(
                        lhs_src,
                        rhs_src,
                        &lhs_tree,
                        &rhs_tree,
                        &arena,
                        lang_config,
                        diff_options,
                    ) {
                        Ok((lhs, rhs)) => {
                            if diff_options.check_only {
                                let has_syntactic_changes = lhs != rhs;

                                let has_byte_changes = if lhs_src == rhs_src {
                                    None
                                } else {
                                    Some((lhs_src.as_bytes().len(), rhs_src.as_bytes().len()))
                                };

                                return DiffResult {
                                    extra_info,
                                    display_path: display_path.to_owned(),
                                    file_format: FileFormat::SupportedLanguage(language),
                                    lhs_src: FileContent::Text(lhs_src.to_owned()),
                                    rhs_src: FileContent::Text(rhs_src.to_owned()),
                                    lhs_positions: vec![],
                                    rhs_positions: vec![],
                                    hunks: vec![],
                                    lhs_folds: vec![],
                                    rhs_folds: vec![],
                                    fold_pairs: vec![],
                                    has_byte_changes,
                                    has_syntactic_changes,
                                };
                            }

                            let mut change_map = ChangeMap::default();
                            let possibly_changed = if env::var("DFT_DBG_KEEP_UNCHANGED").is_ok() {
                                vec![(lhs.clone(), rhs.clone())]
                            } else {
                                unchanged::mark_unchanged(&lhs, &rhs, &mut change_map)
                            };

                            let mut exceeded_graph_limit = false;

                            for (lhs_section_nodes, rhs_section_nodes) in possibly_changed {
                                init_next_prev(&lhs_section_nodes);
                                init_next_prev(&rhs_section_nodes);

                                match mark_syntax(
                                    lhs_section_nodes.first().copied(),
                                    rhs_section_nodes.first().copied(),
                                    &mut change_map,
                                    diff_options.graph_limit,
                                ) {
                                    Ok(()) => {}
                                    Err(ExceededGraphLimit {}) => {
                                        exceeded_graph_limit = true;
                                        break;
                                    }
                                }
                            }

                            if exceeded_graph_limit {
                                // The parse still stands: folds and enclosing
                                // context come from it, and the line diff
                                // supplies the alignment they hang off.
                                folds::unmatched(&lhs, &mut lhs_folds);
                                folds::unmatched(&rhs, &mut rhs_folds);
                                annotations = display::syntax_context::SyntaxAnnotations::collect(
                                    (&lhs, &rhs),
                                );
                                let (lhs_positions, rhs_positions) =
                                    line_parser::change_positions(lhs_src, rhs_src);
                                (
                                    FileFormat::TextFallback {
                                        reason: format!(
                                            "structural diff exceeded diff.graph_limit ({}); raise it in diffr config",
                                            diff_options.graph_limit
                                        ),
                                    },
                                    lhs_positions,
                                    rhs_positions,
                                )
                            } else {
                                fix_all_sliders(language, &lhs, &mut change_map);
                                fix_all_sliders(language, &rhs, &mut change_map);

                                let mut lhs_positions =
                                    syntax::change_positions(&lhs, &change_map, &mut lhs_folds);
                                let mut rhs_positions =
                                    syntax::change_positions(&rhs, &change_map, &mut rhs_folds);

                                if diff_options.ignore_comments {
                                    let lhs_comments =
                                        tsp::comment_positions(&lhs_tree, lhs_src, lang_config);
                                    lhs_positions.extend(lhs_comments);

                                    let rhs_comments =
                                        tsp::comment_positions(&rhs_tree, rhs_src, lang_config);
                                    rhs_positions.extend(rhs_comments);
                                }

                                annotations = display::syntax_context::SyntaxAnnotations::collect(
                                    (&lhs, &rhs),
                                );

                                (
                                    FileFormat::SupportedLanguage(language),
                                    lhs_positions,
                                    rhs_positions,
                                )
                            }
                        }
                        Err(tsp::ExceededParseErrorLimit {
                            error_count,
                            first_error_pos,
                        }) => {
                            let location = match first_error_pos {
                                Some((line, column, side)) => {
                                    let in_initial = match side {
                                        Side::Left => " in initial file",
                                        Side::Right => "",
                                    };
                                    format!(
                                        ", first at {}:{}{}",
                                        line.display(),
                                        column,
                                        in_initial
                                    )
                                }
                                None => "".to_owned(),
                            };
                            let file_format = FileFormat::TextFallback {
                                reason: format!(
                                    "{} {} parse error{}, exceeded DFT_PARSE_ERROR_LIMIT{}",
                                    error_count,
                                    language_name(language),
                                    if error_count == 1 { "" } else { "s" },
                                    location
                                ),
                            };

                            if diff_options.check_only {
                                return check_only_text(
                                    &file_format,
                                    display_path,
                                    extra_info,
                                    lhs_src,
                                    rhs_src,
                                );
                            }

                            // The trees parsed, only with too many errors to
                            // match on. Folds and context still come from them.
                            let (lhs, _) = tsp::to_syntax(
                                &lhs_tree,
                                lhs_src,
                                &arena,
                                lang_config,
                                diff_options.ignore_comments,
                            );
                            let (rhs, _) = tsp::to_syntax(
                                &rhs_tree,
                                rhs_src,
                                &arena,
                                lang_config,
                                diff_options.ignore_comments,
                            );
                            folds::unmatched(&lhs, &mut lhs_folds);
                            folds::unmatched(&rhs, &mut rhs_folds);
                            annotations =
                                display::syntax_context::SyntaxAnnotations::collect((&lhs, &rhs));

                            let (lhs_positions, rhs_positions) =
                                line_parser::change_positions(lhs_src, rhs_src);
                            (file_format, lhs_positions, rhs_positions)
                        }
                    }
                }
                Err(tsp::ExceededByteLimit(num_bytes)) => {
                    let format_options = FormatSizeOptions::from(BINARY).decimal_places(1);
                    let file_format = FileFormat::TextFallback {
                        reason: format!(
                            "{} exceeded diff.byte_limit ({}); raise it in diffr config",
                            format_size(num_bytes, format_options),
                            diff_options.byte_limit
                        ),
                    };

                    if diff_options.check_only {
                        return check_only_text(
                            &file_format,
                            display_path,
                            extra_info,
                            lhs_src,
                            rhs_src,
                        );
                    }

                    let (lhs_positions, rhs_positions) =
                        line_parser::change_positions(lhs_src, rhs_src);
                    (file_format, lhs_positions, rhs_positions)
                }
            }
        }
    };

    let opposite_to_lhs = opposite_positions(&lhs_positions);
    let opposite_to_rhs = opposite_positions(&rhs_positions);

    let hunks = matched_pos_to_hunks(&lhs_positions, &rhs_positions);
    let hunks = merge_adjacent(
        &hunks,
        &opposite_to_lhs,
        &opposite_to_rhs,
        lhs_src.max_line(),
        rhs_src.max_line(),
        display_options.num_context_lines as usize,
    );
    let has_syntactic_changes = !hunks.is_empty();
    let hunks = display::prepare::prepare(
        &hunks,
        (lhs_src, rhs_src),
        (&lhs_positions, &rhs_positions),
        &annotations,
        display_options.num_context_lines as usize,
    );

    let has_byte_changes = if lhs_src == rhs_src {
        None
    } else {
        Some((lhs_src.as_bytes().len(), rhs_src.as_bytes().len()))
    };

    let fold_pairs = match file_format {
        FileFormat::SupportedLanguage(_) => folds::pair_matched(&lhs_folds, &rhs_folds),
        _ => line_folds::pair(
            &line_folds::fallback_rows((lhs_src, rhs_src), (&lhs_positions, &rhs_positions)),
            (lhs_src, rhs_src),
            (&lhs_folds, &rhs_folds),
        ),
    };

    DiffResult {
        extra_info,
        display_path: display_path.to_owned(),
        file_format,
        lhs_src: FileContent::Text(lhs_src.to_owned()),
        rhs_src: FileContent::Text(rhs_src.to_owned()),
        lhs_positions,
        rhs_positions,
        hunks,
        lhs_folds,
        rhs_folds,
        fold_pairs,
        has_byte_changes,
        has_syntactic_changes,
    }
}
