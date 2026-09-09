//! Inline, or "unified" diff display.

use line_numbers::LineNumber;

use crate::constants::Side;
use crate::display::hunks::Hunk;
use crate::display::style::{self, apply_colors, apply_line_number_color};
use crate::lines::{format_line_num, format_line_num_padded, split_on_newlines, MaxLine};
use crate::options::DisplayOptions;
use crate::parse::syntax::MatchedPos;
use crate::summary::FileFormat;

pub(crate) fn print(
    lhs_src: &str,
    rhs_src: &str,
    display_options: &DisplayOptions,
    lhs_mps: &[MatchedPos],
    rhs_mps: &[MatchedPos],
    hunks: &[Hunk],
    display_path: &str,
    extra_info: &Option<String>,
    file_format: &FileFormat,
) {
    let (lhs_colored_lines, rhs_colored_lines) = if display_options.use_color {
        (
            apply_colors(
                lhs_src,
                Side::Left,
                display_options.syntax_highlight,
                file_format,
                display_options.background_color,
                lhs_mps,
            ),
            apply_colors(
                rhs_src,
                Side::Right,
                display_options.syntax_highlight,
                file_format,
                display_options.background_color,
                rhs_mps,
            ),
        )
    } else {
        (
            split_on_newlines(lhs_src)
                .map(|s| format!("{}\n", s))
                .collect(),
            split_on_newlines(rhs_src)
                .map(|s| format!("{}\n", s))
                .collect(),
        )
    };

    let lhs_colored_lines: Vec<_> = lhs_colored_lines
        .into_iter()
        .map(|line| style::replace_tabs(&line, display_options.tab_width))
        .collect();
    let rhs_colored_lines: Vec<_> = rhs_colored_lines
        .into_iter()
        .map(|line| style::replace_tabs(&line, display_options.tab_width))
        .collect();

    // Calculate the maximum line number width for alignment
    let lhs_line_nums_width = format_line_num(lhs_src.max_line()).len();
    let rhs_line_nums_width = format_line_num(rhs_src.max_line()).len();

    let lhs_lines: Vec<_> = split_on_newlines(lhs_src).collect();
    let rhs_lines: Vec<_> = split_on_newlines(rhs_src).collect();
    for (i, hunk) in hunks.iter().enumerate() {
        println!(
            "{}",
            style::header(
                display_path,
                extra_info.as_ref(),
                i + 1,
                hunks.len(),
                file_format,
                display_options
            )
        );

        let mut removed = String::new();
        let mut added = String::new();
        let mut previous: (Option<LineNumber>, Option<LineNumber>) = (None, None);
        for &(lhs, rhs) in &hunk.lines {
            let has_gap = lhs
                .zip(previous.0)
                .is_some_and(|(line, prev)| line.0 > prev.0 + 1)
                || rhs
                    .zip(previous.1)
                    .is_some_and(|(line, prev)| line.0 > prev.0 + 1);
            if has_gap {
                println!("{}{}      ...", removed, added);
                removed.clear();
                added.clear();
            }
            if lhs.is_some() {
                previous.0 = lhs;
            }
            if rhs.is_some() {
                previous.1 = rhs;
            }
            if let (Some(left), Some(right)) = (lhs, rhs) {
                if lhs_lines[left.as_usize()] == rhs_lines[right.as_usize()] {
                    print!("{}{}", removed, added);
                    removed.clear();
                    added.clear();
                    print!(
                        "{}   {}",
                        apply_line_number_color(
                            &format_line_num_padded(left, lhs_line_nums_width),
                            false,
                            Side::Left,
                            display_options
                        ),
                        lhs_colored_lines[left.as_usize()]
                    );
                    continue;
                }
            }
            if let Some(line) = lhs {
                removed.push_str(&format!(
                    "{}   {}",
                    apply_line_number_color(
                        &format_line_num_padded(line, lhs_line_nums_width),
                        hunk.novel_lhs.contains(&line),
                        Side::Left,
                        display_options
                    ),
                    lhs_colored_lines[line.as_usize()]
                ));
            }
            if let Some(line) = rhs {
                added.push_str(&format!(
                    "   {}{}",
                    apply_line_number_color(
                        &format_line_num_padded(line, rhs_line_nums_width),
                        hunk.novel_rhs.contains(&line),
                        Side::Right,
                        display_options
                    ),
                    rhs_colored_lines[line.as_usize()]
                ));
            }
        }
        print!("{}{}", removed, added);
        println!();
    }
}
