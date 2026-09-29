//! The pack builder and runtime registry share languages/extra.json.
use super::guess_language::Language;
use super::tree_sitter_parser::TreeSitterConfig;
use crate::languages::ParserError;
use crate::summary::FallbackCause;

macro_rules! extra_languages {
    ($( $variant:ident => {
        id: $id:literal, feature: $feature:literal, parser: $parser:path,
        highlights: [$($query:expr),*], atoms: [$($atom:literal),*],
        delimiters: [$($delimiter:expr),*], ignore_trailing: [$($trailing:expr),*]
    }, )*) => {
        pub(crate) fn id(language: Language) -> Option<&'static str> {
            match language { $(Language::$variant => Some($id),)* _ => None }
        }
        #[allow(clippy::match_like_matches_macro)]
        pub(crate) fn builtin(language: Language) -> bool {
            match language { $(Language::$variant => cfg!(feature = $feature),)* _ => true }
        }
        pub(crate) fn config(language: Language) -> Result<Option<TreeSitterConfig>, ParserError> {
            match language {
                $(Language::$variant => {
                    #[cfg(feature = $feature)]
                    let compiled = {
                        let queries: &[&str] = &[$($query),*];
                        Some((tree_sitter::Language::new($parser), queries.concat()))
                    };
                    #[cfg(not(feature = $feature))]
                    let compiled = None;
                    let (language, highlights) = match compiled {
                        Some(parser) => parser,
                        None => super::native::load($id)?,
                    };
                    let highlight_query = tree_sitter::Query::new(&language, &highlights)
                        .map_err(|error| ParserError::new(FallbackCause::ParserLoadFailed, error))?;
                    Ok(Some(TreeSitterConfig {
                        language,
                        highlight_query,
                        atom_nodes: [$($atom),*].into_iter().collect(),
                        delimiter_tokens: vec![$($delimiter),*],
                        ignore_trailing_tokens: vec![$($trailing),*],
                        sub_languages: vec![],
                    }))
                },)*
                _ => Ok(None),
            }
        }
    };
}

include!(concat!(env!("OUT_DIR"), "/extra_languages.rs"));
