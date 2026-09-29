//! A grammar's code must outlive every Tree-sitter language, parser and tree.
use crate::hash::DftHashMap;
use crate::languages::{self, ParserError};
use crate::summary::FallbackCause;
use anyhow::{ensure, Context};
use libloading::Library;
use std::sync::{LazyLock, Mutex};
use tree_sitter_language::LanguageFn;

struct Loaded {
    language: tree_sitter::Language,
    _library: Library,
}

pub(crate) fn load(id: &'static str) -> Result<tree_sitter::Language, ParserError> {
    static LOADED: LazyLock<Mutex<DftHashMap<&'static str, Loaded>>> =
        LazyLock::new(|| Mutex::new(DftHashMap::default()));
    let mut loaded = LOADED.lock().unwrap();
    if let Some(grammar) = loaded.get(id) {
        return Ok(grammar.language.clone());
    }
    let package = languages::package().ok_or_else(|| {
        ParserError::new(
            FallbackCause::ParserUnavailable,
            format!(
                "{id}: no compatible published pack for {}",
                languages::TARGET
            ),
        )
    })?;
    let directory = languages::directory(package)
        .map_err(|e| ParserError::new(FallbackCause::ParserLoadFailed, e))?;
    if !directory.exists() {
        return Err(ParserError::new(
            FallbackCause::ParserNotInstalled,
            format!("{id} parser is not installed"),
        ));
    }
    let result = (|| -> anyhow::Result<Loaded> {
        let _lock = languages::lock(package)?;
        let entry = package
            .libraries
            .iter()
            .find(|l| l.id == id)
            .context("missing catalog language")?;
        ensure!(
            (tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION..=tree_sitter::LANGUAGE_VERSION)
                .contains(&entry.abi),
            "incompatible catalog ABI"
        );
        let path = languages::verify_library(package, entry, &directory)?;
        // Only exact, catalog-hashed code is loaded. The handle stays in LOADED forever.
        let library = unsafe { Library::new(path)? };
        let function =
            unsafe { library.get::<unsafe extern "C" fn() -> *const ()>(entry.symbol.as_bytes())? };
        ensure!(!unsafe { function() }.is_null(), "null grammar pointer");
        let language = tree_sitter::Language::new(unsafe { LanguageFn::from_raw(*function) });
        ensure!(
            language.abi_version() == entry.abi,
            "grammar ABI differs from catalog"
        );
        tree_sitter::Parser::new().set_language(&language)?;
        Ok(Loaded {
            language,
            _library: library,
        })
    })()
    .map_err(|e| ParserError::new(FallbackCause::ParserLoadFailed, format!("{id}: {e:#}")))?;
    let language = result.language.clone();
    loaded.insert(id, result);
    Ok(language)
}
