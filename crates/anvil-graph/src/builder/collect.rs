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
            if matches!(
                name,
                ".git" | ".godot" | ".harness" | "node_modules" | "target" | "dist" | "build"
                    | "coverage" | "graphify-out" | ".dart_tool" | ".gradle"
                    | "__pycache__" | ".venv" | "venv" | "out-tsc" | ".next" | ".nuxt"
                    | ".svelte-kit" | ".astro" | "vendor" | "storage"
            ) {
                return false;
            }
            let norm = entry.path().to_string_lossy().replace('\\', "/");
            if norm.ends_with("/bootstrap/cache") || norm == "bootstrap/cache" {
                return false;
            }
            return true;
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
    if matches!(
        anvil_ast::detect_language(path),
        Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Jsx
            | Language::Html | Language::Css | Language::Scss | Language::Python
            | Language::Kotlin | Language::Dart | Language::Rust
            | Language::Php | Language::Blade
            | Language::GdScript | Language::GodotScene | Language::GdShader
    ) {
        return true;
    }
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("vue")
                || ext.eq_ignore_ascii_case("svelte")
                || ext.eq_ignore_ascii_case("astro")
        })
}
