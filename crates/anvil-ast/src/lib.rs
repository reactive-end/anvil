//! Embedded `tree-sitter` AST parsing, symbol/edge extraction, and Tier 0 rules.

pub mod extractors;
pub mod godot;
pub mod parser;
pub mod rules;
pub mod sfc;

use std::path::Path;

use anyhow::Result;
use anvil_core::config::AnvilConfig;
use anvil_core::traits::{FileAstFacts, Language, LanguageExtractor};
use rayon::prelude::*;

/// Crate version marker for workspace smoke checks.
pub fn crate_name() -> &'static str {
    "anvil-ast"
}

/// Detects the programming or markup language from a file path extension.
pub fn detect_language(path: &Path) -> Language {
    if let Some(name) = path.file_name().and_then(|n| n.to_str())
        && name.to_ascii_lowercase().ends_with(".blade.php")
    {
        return Language::Blade;
    }
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return Language::Other;
    };

    match ext.to_ascii_lowercase().as_str() {
        "ts" | "mts" | "cts" => Language::TypeScript,
        "tsx" => Language::Tsx,
        "js" | "mjs" | "cjs" => Language::JavaScript,
        "jsx" => Language::Jsx,
        "html" | "htm" => Language::Html,
        "css" => Language::Css,
        "scss" => Language::Scss,
        "py" | "pyi" => Language::Python,
        "kt" | "kts" => Language::Kotlin,
        "dart" => Language::Dart,
        "rs" => Language::Rust,
        "php" => Language::Php,
        "gd" => Language::GdScript,
        "tscn" | "tres" => Language::GodotScene,
        "gdshader" => Language::GdShader,
        "json" => Language::Json,
        "yaml" | "yml" => Language::Yaml,
        "md" | "mdx" => Language::Markdown,
        _ => Language::Other,
    }
}

/// Parses `source` ONCE with a thread-local `tree-sitter` parser and extracts
/// symbols, dependency edges, and Tier 0 diagnostics in a single AST walk.
pub fn analyze_file(path: &Path, source: &str, config: &AnvilConfig) -> Result<FileAstFacts> {
    let blake3_hash = blake3::hash(source.as_bytes()).to_hex().to_string();
    let normalized_file = normalize_path(path);
    if sfc::is_sfc_path(path) {
        return sfc::analyze_sfc(path, &normalized_file, source, blake3_hash, config);
    }

    let language = detect_language(path);
    if matches!(
        language,
        Language::GdScript | Language::GodotScene | Language::GdShader
    ) {
        return Ok(godot::analyze_godot(
            path,
            &normalized_file,
            source,
            blake3_hash,
            language,
            config,
        ));
    }

    let tree = parser::parse_source(language, source)?;

    Ok(extractors::collect_file_facts(
        path,
        &normalized_file,
        source,
        blake3_hash,
        language,
        tree.as_ref(),
        config,
    ))
}

/// Analyzes multiple `(path, source)` pairs in parallel using `rayon` and per-thread parser pools.
pub fn analyze_files_parallel<P, S>(
    files: &[(P, S)],
    config: &AnvilConfig,
) -> Vec<Result<FileAstFacts>>
where
    P: AsRef<Path> + Sync,
    S: AsRef<str> + Sync,
{
    files
        .par_iter()
        .map(|(path, source)| analyze_file(path.as_ref(), source.as_ref(), config))
        .collect()
}

/// Adapter implementing `anvil_core::traits::LanguageExtractor` using default `AnvilConfig`.
pub struct TreeSitterExtractor {
    language: Language,
}

impl TreeSitterExtractor {
    /// Creates an extractor bound to a specific `Language`.
    pub fn new(language: Language) -> Self {
        Self { language }
    }
}

impl LanguageExtractor for TreeSitterExtractor {
    fn language(&self) -> Language {
        self.language
    }

    fn extract(&self, path: &Path, source: &str) -> Result<FileAstFacts> {
        analyze_file(path, source, &AnvilConfig::default())
    }
}

fn normalize_path(path: &Path) -> String {
    let raw = path.to_string_lossy();
    if raw.contains('\\') {
        raw.replace('\\', "/")
    } else {
        raw.into_owned()
    }
}
