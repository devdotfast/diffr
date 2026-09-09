pub(crate) mod context;
pub(crate) mod folds;
pub(crate) mod guess_language;
pub(crate) mod syntax;
pub(crate) mod tree_sitter_parser;

pub(crate) fn query_source(language: guess_language::Language) -> Option<&'static str> {
    use guess_language::Language;
    Some(match language {
        Language::Rust => include_str!("syntax_queries/rust.scm"),
        Language::Python => include_str!("syntax_queries/python.scm"),
        Language::Go => include_str!("syntax_queries/go.scm"),
        Language::JavaScript
        | Language::JavascriptJsx
        | Language::TypeScript
        | Language::TypeScriptTsx => include_str!("syntax_queries/javascript.scm"),
        _ => return None,
    })
}
