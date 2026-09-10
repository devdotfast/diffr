//! Compile the supported Tree-sitter capture and directive conventions.
use super::ConfigError;
use tree_sitter::{Query, QueryPredicateArg};

pub(crate) struct AnnotationQuery {
    pub(crate) query: Query,
    pub(crate) patterns: Vec<Pattern>,
}

#[derive(Default)]
pub(crate) struct Pattern {
    pub(crate) tags: Vec<String>,
    pub(crate) offsets: Vec<Offset>,
    pub(crate) ranges: Vec<CaptureRange>,
}

pub(crate) struct CaptureRange {
    pub(crate) target: u32,
    pub(crate) start: u32,
    pub(crate) end: u32,
}

pub(crate) struct Offset {
    pub(crate) capture: u32,
    pub(crate) start_row: isize,
    pub(crate) start_column: isize,
    pub(crate) end_row: isize,
    pub(crate) end_column: isize,
}

impl AnnotationQuery {
    pub(crate) fn compile(
        grammar: &tree_sitter::Language,
        source: &str,
    ) -> Result<Self, ConfigError> {
        let query = Query::new(grammar, source).map_err(|error| ConfigError(error.to_string()))?;
        for name in query.capture_names() {
            if name.starts_with('_')
                || matches!(
                    *name,
                    "fold"
                        | "context"
                        | "context.start"
                        | "context.end"
                        | "context.final"
                        | "context.last"
                )
            {
                continue;
            }
            return Err(ConfigError(format!(
                "unsupported capture @{name}; use an underscore prefix for helper captures"
            )));
        }
        let mut patterns = Vec::new();
        for index in 0..query.pattern_count() {
            let mut pattern = Pattern::default();
            if !query.property_predicates(index).is_empty() {
                return Err(ConfigError("#is? and #is-not? are not supported".into()));
            }
            for property in query.property_settings(index) {
                if property.key.as_ref() != "tag" {
                    return Err(ConfigError(format!(
                        "unsupported #set! property {}",
                        property.key
                    )));
                }
                if property
                    .capture_id
                    .is_some_and(|id| query.capture_names()[id] != "fold")
                {
                    return Err(ConfigError(
                        "tag metadata must target @fold or its pattern".into(),
                    ));
                }
                let tag = property
                    .value
                    .as_deref()
                    .filter(|tag| !tag.is_empty())
                    .ok_or_else(|| ConfigError("#set! tag requires a nonempty string".into()))?;
                pattern.tags.push(tag.to_owned());
            }
            for predicate in query.general_predicates(index) {
                if predicate.operator.as_ref() == "make-range!" {
                    let [QueryPredicateArg::String(name), QueryPredicateArg::Capture(start), QueryPredicateArg::Capture(end)] =
                        predicate.args.as_ref()
                    else {
                        return Err(ConfigError(
                            "#make-range! requires a capture name and two boundary captures".into(),
                        ));
                    };
                    let target = query.capture_index_for_name(name).ok_or_else(|| {
                        ConfigError(format!("#make-range! target @{name} must be captured"))
                    })?;
                    if pattern.ranges.iter().any(|range| range.target == target) {
                        return Err(ConfigError(
                            "duplicate #make-range! target in one pattern".into(),
                        ));
                    }
                    pattern.ranges.push(CaptureRange {
                        target,
                        start: *start,
                        end: *end,
                    });
                    continue;
                }
                if predicate.operator.as_ref() != "offset!" {
                    return Err(ConfigError(format!(
                        "unsupported directive #{}",
                        predicate.operator
                    )));
                }
                let [QueryPredicateArg::Capture(capture), a, b, c, d] = predicate.args.as_ref()
                else {
                    return Err(ConfigError(
                        "#offset! requires a capture and four integer offsets".into(),
                    ));
                };
                let integer = |arg: &QueryPredicateArg| -> Result<isize, ConfigError> {
                    let QueryPredicateArg::String(value) = arg else {
                        return Err(ConfigError("offsets must be integers".into()));
                    };
                    value
                        .parse()
                        .map_err(|_| ConfigError(format!("invalid offset: {value}")))
                };
                if pattern
                    .offsets
                    .iter()
                    .any(|offset: &Offset| offset.capture == *capture)
                {
                    return Err(ConfigError(
                        "duplicate #offset! for a capture in one pattern".into(),
                    ));
                }
                pattern.offsets.push(Offset {
                    capture: *capture,
                    start_row: integer(a)?,
                    start_column: integer(b)?,
                    end_row: integer(c)?,
                    end_column: integer(d)?,
                });
            }
            patterns.push(pattern);
        }
        Ok(Self { query, patterns })
    }
}
