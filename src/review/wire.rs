//! Lossless JSON encoding of the domain model, not Difftastic's display JSON.
use crate::display::line_layout as layout;
use crate::lines::SourceRange;
use crate::parse::folds::Correspondence;
use crate::parse::syntax::{MatchKind, MatchedPos};
use crate::summary::DiffResult;
use crate::summary::{FileContent, FileFormat};
use line_numbers::SingleLineSpan;
use serde_json::{json, Value};

fn span(position: &SingleLineSpan) -> Value {
    json!({
        "line": position.line.0,
        "start_col": position.start_col,
        "end_col": position.end_col,
    })
}

fn spans(positions: &[SingleLineSpan]) -> Value {
    Value::Array(positions.iter().map(span).collect())
}

fn position(position: &MatchedPos) -> Value {
    let kind = match &position.kind {
        MatchKind::UnchangedToken {
            highlight,
            self_pos,
            opposite_pos,
        } => json!({
            "UnchangedToken": {
                "highlight": highlight,
                "self_pos": spans(self_pos),
                "opposite_pos": spans(opposite_pos),
            },
        }),
        MatchKind::UnchangedPartOfNovelItem {
            highlight,
            self_pos,
            opposite_pos,
        } => json!({
            "UnchangedPartOfNovelItem": {
                "highlight": highlight,
                "self_pos": span(self_pos),
                "opposite_pos": spans(opposite_pos),
            },
        }),
        MatchKind::Novel { highlight } => json!({"Novel": {"highlight": highlight}}),
        MatchKind::NovelWord { highlight } => json!({"NovelWord": {"highlight": highlight}}),
        MatchKind::Ignored { highlight } => json!({"Ignored": {"highlight": highlight}}),
    };
    json!({"pos": span(&position.pos), "kind": kind})
}

fn range(range: &SourceRange) -> Value {
    json!({
        "start": {
            "line": range.start.line.0,
            "byte_column": range.start.byte_column,
        },
        "end": {
            "line": range.end.line.0,
            "byte_column": range.end.byte_column,
        },
    })
}

fn correspondence(region: &Correspondence<SourceRange>) -> Value {
    match region {
        Correspondence::Paired { lhs, rhs } => json!({
            "Paired": {"lhs": range(lhs), "rhs": range(rhs)},
        }),
        Correspondence::Added(rhs) => json!({"Added": range(rhs)}),
        Correspondence::Deleted(lhs) => json!({"Deleted": range(lhs)}),
    }
}

fn content(content: &FileContent) -> Value {
    match content {
        FileContent::Text(source) => json!({"Text": source}),
        FileContent::Binary => json!("Binary"),
    }
}

fn file_format(format: &FileFormat) -> Value {
    match format {
        FileFormat::SupportedLanguage(language) => json!({
            "SupportedLanguage": crate::parse::guess_language::language_name(*language),
        }),
        FileFormat::PlainText => json!("PlainText"),
        FileFormat::Binary => json!("Binary"),
        FileFormat::TextFallback { reason } => json!({"TextFallback": {"reason": reason}}),
    }
}

fn hunk(hunk: &crate::display::hunks::Hunk) -> Value {
    let mut novel_lhs: Vec<_> = hunk.novel_lhs.iter().map(|line| line.0).collect();
    let mut novel_rhs: Vec<_> = hunk.novel_rhs.iter().map(|line| line.0).collect();
    novel_lhs.sort_unstable();
    novel_rhs.sort_unstable();
    let lines: Vec<_> = hunk
        .lines
        .iter()
        .map(|(lhs, rhs)| (lhs.map(|line| line.0), rhs.map(|line| line.0)))
        .collect();
    json!({
        "novel_lhs": novel_lhs, "novel_rhs": novel_rhs, "lines": lines,
    })
}

impl DiffResult {
    pub(crate) fn domain_json(&self) -> Value {
        let hunks: Vec<_> = self.hunks.iter().map(hunk).collect();
        let folds: Vec<_> = self
            .folds
            .iter()
            .map(|fold| {
                json!({
                    "kind": fold.kind,
                    "regions": correspondence(&fold.regions),
                    "placeholder": fold.placeholder,
                })
            })
            .collect();
        json!({
            "display_path": self.display_path,
            "extra_info": self.extra_info,
            "file_format": file_format(&self.file_format),
            "lhs_src": content(&self.lhs_src),
            "rhs_src": content(&self.rhs_src),
            "lhs_positions": self.lhs_positions.iter().map(position).collect::<Vec<_>>(),
            "rhs_positions": self.rhs_positions.iter().map(position).collect::<Vec<_>>(),
            "hunks": hunks,
            "line_alignment": self.line_alignment,
            "has_byte_changes": self.has_byte_changes,
            "has_syntactic_changes": self.has_syntactic_changes,
            "folds": folds,
        })
    }

    /// Adapt the existing alignment and selected rows for the fixture viewer.
    pub(crate) fn viewer_json(&self) -> Value {
        let rows = &self.line_alignment;
        let baseline = layout::LineSelection::from_hunks(&self.hunks);
        let reindented = layout::reindented_pairs(self);
        json!({
            "domain": self.domain_json(),
            "layout": {"rows": rows, "baseline": (&baseline.lhs, &baseline.rhs), "reindented": reindented},
        })
    }
}
