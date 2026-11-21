//! `.gitignore`-aware repository source file walker using `ignore`.

use std::path::{Path, PathBuf};

use anvil_core::traits::Language;
use ignore::WalkBuilder;

use crate::model::CodeGraph;

/// Collects all supported source files under `repo_root` respecting `.gitignore`.
pub fn collect_source_files(repo_root: &Path) -> Vec<(PathBuf, String)> {
    let mut walker = WalkBuilder::new(repo_root);
    walker.hidden(true).git_ignore(true).git_global(false).git_exclude(true);
    walker.filter_entry(|entry| {
        if entry.depth() == 0 {
            return true;
        }
        if entry.file_type().is_some_and(|ft| ft.is_dir()) {
            let name = entry.file_name().to_str().unwrap_or("");
            return !matches!(
                name,
                ".git" | ".harness" | "node_modules" | "target" | "dist" | "build"
                    | "coverage" | "graphify-out" | ".dart_tool" | ".gradle"
                    | "__pycache__" | ".venv" | "venv" | "out-tsc" | ".next" | ".nuxt"
            );
        }
        true
    });

    let mut out = Vec::new();
    for entry in walker.build().flatten() {
        if !entry.file_type().is_some_and(|ft| ft.is_file()) {
            continue;
        }
        let path = entry.path();
        if is_supported_source(path)
            && let Ok(rel) = path.strip_prefix(repo_root)
        {
            out.push((
                path.to_path_buf(),
                CodeGraph::normalize_rel_path(&rel.to_string_lossy()),
            ));
        }
    }
    out
}

fn is_supported_source(path: &Path) -> bool {
    matches!(
        anvil_ast::detect_language(path),
        Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Jsx
            | Language::Html | Language::Css | Language::Scss | Language::Python
            | Language::Kotlin | Language::Dart
    )
}
