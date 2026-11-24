//! Thread-local `tree-sitter` parser pool keyed by `Language`.

use std::cell::RefCell;

use anyhow::{Context, Result};
use anvil_core::traits::Language;
use rustc_hash::FxHashMap;
use tree_sitter::{Language as TsLanguage, Parser, Tree};

thread_local! {
    static PARSER_POOL: RefCell<FxHashMap<Language, Parser>> = RefCell::new(FxHashMap::default());
}

/// Returns the embedded `tree-sitter` language grammar for `lang`, if supported.
pub fn language_for(lang: Language) -> Option<TsLanguage> {
    match lang {
        Language::TypeScript => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        Language::Tsx => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        Language::JavaScript | Language::Jsx => Some(tree_sitter_javascript::LANGUAGE.into()),
        Language::Html | Language::Blade => Some(tree_sitter_html::LANGUAGE.into()),
        Language::Css | Language::Scss => Some(tree_sitter_css::LANGUAGE.into()),
        Language::Python => Some(tree_sitter_python::LANGUAGE.into()),
        Language::Kotlin => Some(tree_sitter_kotlin_ng::LANGUAGE.into()),
        Language::Dart => Some(tree_sitter_dart::LANGUAGE.into()),
        Language::Rust => Some(tree_sitter_rust::LANGUAGE.into()),
        Language::Php => Some(tree_sitter_php::LANGUAGE_PHP.into()),
        Language::GdScript
        | Language::GodotScene
        | Language::GdShader
        | Language::Json
        | Language::Yaml
        | Language::Markdown
        | Language::Other => None,
    }
}

/// Parses `source` once using a thread-local cached `Parser` for `lang`.
///
/// Returns `Ok(None)` for languages without an embedded grammar.
pub fn parse_source(lang: Language, source: &str) -> Result<Option<Tree>> {
    let Some(ts_lang) = language_for(lang) else {
        return Ok(None);
    };

    PARSER_POOL.with(|pool| {
        let mut map = pool.borrow_mut();
        let parser = match map.entry(lang) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let mut new_parser = Parser::new();
                new_parser
                    .set_language(&ts_lang)
                    .with_context(|| format!("failed to load tree-sitter grammar for {lang:?}"))?;
                entry.insert(new_parser)
            }
        };

        let tree = parser
            .parse(source, None)
            .with_context(|| format!("tree-sitter failed to parse {lang:?} source"))?;
        Ok(Some(tree))
    })
}

/// Returns the number of cached parsers on the current thread (useful in tests).
pub fn cached_parser_count() -> usize {
    PARSER_POOL.with(|pool| pool.borrow().len())
}
