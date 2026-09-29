//! Optional parser identity and compile-time availability.
use super::guess_language::Language;

macro_rules! optional_languages {
    ($( $variant:ident => ($id:literal, $feature:literal), )*) => {
        pub(crate) fn id(language: Language) -> Option<&'static str> {
            match language { $(Language::$variant => Some($id),)* _ => None }
        }
        #[allow(clippy::match_like_matches_macro)]
        pub(crate) fn builtin(language: Language) -> bool {
            match language { $(Language::$variant => cfg!(feature = $feature),)* _ => true }
        }
    };
}

optional_languages! {
    Apex => ("apex", "lang-apex"),
    Fortran => ("fortran", "lang-fortran"),
    FSharp => ("fsharp", "lang-fsharp"),
    Haskell => ("haskell", "lang-haskell"),
    Julia => ("julia", "lang-julia"),
    OCaml => ("ocaml", "lang-ocaml"),
    OCamlInterface => ("ocaml-interface", "lang-ocaml"),
    Qml => ("qml", "lang-qml"),
    Verilog => ("verilog", "lang-verilog"),
    Vhdl => ("vhdl", "lang-vhdl"),
}
