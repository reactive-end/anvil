//! Fast diff classification mapping changed files and hunks to verification Tiers.

use std::sync::OnceLock;

use regex::Regex;
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};

use crate::traits::{DiffCategory, Tier};

/// Input unit representing a single modified file and optional unified diff hunk text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedFile {
    pub path: String,
    pub hunks: Option<String>,
}

impl ChangedFile {
    pub fn from_path(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            hunks: None,
        }
    }

    pub fn with_hunks(path: impl Into<String>, hunks: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            hunks: Some(hunks.into()),
        }
    }
}

/// Per-file classification detail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileClassification {
    pub path: String,
    pub category: DiffCategory,
    pub tier: Tier,
}

/// Aggregated diff classification driving Tier 0 / Tier 1 / Tier 2 execution budgets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffClassification {
    pub primary_category: DiffCategory,
    pub minimum_tier: Tier,
    pub categories: Vec<DiffCategory>,
    pub skip_logic_tests: bool,
    pub needs_visual_qa: bool,
    pub needs_deps_audit: bool,
    pub per_file: Vec<FileClassification>,
}

/// Convenience classifier when only file paths are available.
pub fn classify_paths(paths: &[&str]) -> DiffClassification {
    let files: Vec<ChangedFile> = paths.iter().map(|p| ChangedFile::from_path(*p)).collect();
    classify_files(&files)
}

/// Classifies changed files and optional hunks into semantic categories and minimum Tier.
pub fn classify_files(files: &[ChangedFile]) -> DiffClassification {
    if files.is_empty() {
        return empty_classification();
    }

    let mut seen = FxHashSet::default();
    let mut per_file = Vec::with_capacity(files.len());
    let mut minimum_tier = Tier::Tier0;
    let mut primary = DiffCategory::Docs;

    for file in files {
        let category = classify_single(file);
        let tier = category_tier(category);
        seen.insert(category);
        if tier > minimum_tier {
            minimum_tier = tier;
        }
        if category_rank(category) > category_rank(primary) {
            primary = category;
        }
        per_file.push(FileClassification {
            path: file.path.clone(),
            category,
            tier,
        });
    }

    let only_docs_or_style = seen
        .iter()
        .all(|c| matches!(c, DiffCategory::Docs | DiffCategory::StyleOnly));
    let needs_visual_qa = seen.contains(&DiffCategory::StyleOnly) || seen.contains(&DiffCategory::Ui);
    let needs_deps_audit = seen.contains(&DiffCategory::Deps);
    let categories = seen.into_iter().collect();

    DiffClassification {
        primary_category: primary,
        minimum_tier,
        categories,
        skip_logic_tests: only_docs_or_style,
        needs_visual_qa,
        needs_deps_audit,
        per_file,
    }
}

fn empty_classification() -> DiffClassification {
    DiffClassification {
        primary_category: DiffCategory::Docs,
        minimum_tier: Tier::Tier0,
        categories: Vec::new(),
        skip_logic_tests: true,
        needs_visual_qa: false,
        needs_deps_audit: false,
        per_file: Vec::new(),
    }
}

fn classify_single(file: &ChangedFile) -> DiffCategory {
    let normalized = file.path.replace('\\', "/").to_ascii_lowercase();
    let file_name = normalized.rsplit('/').next().unwrap_or(&normalized);

    if is_docs_path(&normalized, file_name) {
        return DiffCategory::Docs;
    }
    if is_deps_manifest(file_name, &normalized) {
        return DiffCategory::Deps;
    }
    if is_style_extension(&normalized) {
        return DiffCategory::StyleOnly;
    }
    if is_contract_path(&normalized) {
        return DiffCategory::Contract;
    }
    if is_config_path(&normalized, file_name) {
        return DiffCategory::Config;
    }
    if is_ui_path(&normalized) {
        if let Some(hunks) = &file.hunks
            && hunks_are_style_only(hunks)
        {
            return DiffCategory::StyleOnly;
        }
        return DiffCategory::Ui;
    }
    DiffCategory::Logic
}

fn category_tier(category: DiffCategory) -> Tier {
    match category {
        DiffCategory::Docs => Tier::Tier0,
        DiffCategory::StyleOnly
        | DiffCategory::Config
        | DiffCategory::Logic
        | DiffCategory::Contract => Tier::Tier1,
        DiffCategory::Ui | DiffCategory::Deps => Tier::Tier2,
    }
}

fn category_rank(category: DiffCategory) -> u8 {
    match category {
        DiffCategory::Docs => 0,
        DiffCategory::StyleOnly => 1,
        DiffCategory::Config => 2,
        DiffCategory::Logic => 3,
        DiffCategory::Contract => 4,
        DiffCategory::Ui => 5,
        DiffCategory::Deps => 6,
    }
}

fn is_docs_path(path: &str, name: &str) -> bool {
    path.starts_with("docs/")
        || path.contains("/docs/")
        || matches!(name, "license" | "readme" | "changelog")
        || path.ends_with(".md")
        || path.ends_with(".mdx")
        || path.ends_with(".txt")
        || path.ends_with(".rst")
}

fn is_deps_manifest(name: &str, path: &str) -> bool {
    matches!(
        name,
        "package.json" | "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock"
            | "cargo.toml" | "cargo.lock" | "pyproject.toml" | "poetry.lock"
            | "uv.lock" | "pubspec.yaml" | "pubspec.lock" | "libs.versions.toml"
            | "build.gradle" | "build.gradle.kts"
    ) || (name.starts_with("requirements") && path.ends_with(".txt"))
}

fn is_style_extension(path: &str) -> bool {
    [".css", ".scss", ".sass", ".less", ".styl"]
        .iter()
        .any(|ext| path.ends_with(ext))
}

fn is_contract_path(path: &str) -> bool {
    [".proto", ".graphql", ".gql", ".prisma"]
        .iter()
        .any(|ext| path.ends_with(ext))
        || ["openapi", "swagger", "/contracts/", "/dto/", "/interfaces/"]
            .iter()
            .any(|seg| path.contains(seg))
}

fn is_config_path(path: &str, name: &str) -> bool {
    path.starts_with(".github/")
        || path.starts_with(".harness/")
        || path.ends_with(".yml")
        || path.ends_with(".yaml")
        || [
            "tsconfig", ".eslintrc", "eslint.config.", "biome.json",
            "vite.config.", "next.config.", "astro.config.", "vitest.config.", "jest.config.",
        ]
        .iter()
        .any(|p| name.starts_with(p))
        || matches!(name, "angular.json" | "nest-cli.json" | "ruff.toml" | ".ruff.toml")
}

fn is_ui_path(path: &str) -> bool {
    [".tsx", ".jsx", ".html", ".vue", ".svelte", "screen.kt", "activity.kt", "_widget.dart"]
        .iter()
        .any(|ext| path.ends_with(ext))
}

fn hunks_are_style_only(hunks: &str) -> bool {
    static STYLE_LINE_RE: OnceLock<Regex> = OnceLock::new();
    let re = STYLE_LINE_RE.get_or_init(|| {
        Regex::new(r#"^[+-]\s*(?:className=|class=|style=|\.?[a-zA-Z0-9_-]+\s*:\s*['"]?#[0-9a-fA-F]{3,8}|color:|background|border-color)"#)
            .expect("valid static style regex")
    });
    let mut saw_change = false;
    for line in hunks.lines() {
        if line.starts_with("+++") || line.starts_with("---") || line.starts_with("@@") {
            continue;
        }
        if line.starts_with('+') || line.starts_with('-') {
            saw_change = true;
            if !re.is_match(line) {
                return false;
            }
        }
    }
    saw_change
}
