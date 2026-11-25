//! Auto-detection of single-repo and multi-repo technology stacks and tooling.

mod ecosystems;

use std::fs;
use std::path::Path;

use anyhow::Result;
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};

use crate::config::{AnvilConfig, StackKind, SubrepoConfig, ToolKind};

/// Detection output for a single directory/repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetectedRepo {
    pub name: String,
    pub rel_path: String,
    pub stacks: Vec<StackKind>,
    pub tools: Vec<ToolKind>,
    #[serde(default)]
    pub has_git: bool,
}

/// Workspace-level detection summary supporting single-repo and multi-repo roots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDetection {
    pub is_multi_repo: bool,
    pub repos: Vec<DetectedRepo>,
}

impl WorkspaceDetection {
    /// Applies detected stacks, tools, and subrepos onto an [`AnvilConfig`] deterministically.
    pub fn apply_to_config(&self, config: &mut AnvilConfig) {
        let _ = self.merge_into_config(config);
    }

    /// Merges newly detected stacks, tools, subrepos, and root allowlist items into `config`.
    /// Returns `true` when `config` was modified.
    pub fn merge_into_config(&self, config: &mut AnvilConfig) -> bool {
        let before = config.clone();
        let existing_sonar: rustc_hash::FxHashMap<String, bool> = config
            .subrepos
            .iter()
            .map(|s| (s.path.clone(), s.sonar_enabled))
            .collect();
        let mut all_stacks: FxHashSet<StackKind> = config.stacks.iter().copied().collect();
        let mut all_tools: FxHashSet<ToolKind> = config.tools.iter().copied().collect();
        config.subrepos.clear();
        for repo in &self.repos {
            let mut stacks = repo.stacks.clone();
            stacks.sort();
            stacks.dedup();
            let mut tools = repo.tools.clone();
            tools.sort();
            tools.dedup();
            all_stacks.extend(stacks.iter().copied());
            all_tools.extend(tools.iter().copied());
            if self.is_multi_repo || repo.rel_path != "." {
                config.subrepos.push(SubrepoConfig {
                    name: repo.name.clone(),
                    path: repo.rel_path.clone(),
                    stacks,
                    tools,
                    sonar_enabled: existing_sonar.get(&repo.rel_path).copied().unwrap_or(false),
                });
            }
        }
        let mut sorted_stacks: Vec<_> = all_stacks.into_iter().collect();
        sorted_stacks.sort();
        config.stacks = sorted_stacks;

        let mut sorted_tools: Vec<_> = all_tools.into_iter().collect();
        sorted_tools.sort();
        config.tools = sorted_tools;

        extend_stack_root_allowlist(config);
        *config != before
    }
}

fn extend_stack_root_allowlist(config: &mut AnvilConfig) {
    let mut extras: Vec<String> = Vec::new();
    let has_js = config.stacks.iter().any(|s| {
        matches!(
            s,
            StackKind::React
                | StackKind::Vue
                | StackKind::Svelte
                | StackKind::Vite
                | StackKind::NextJs
                | StackKind::Astro
                | StackKind::Angular
                | StackKind::NestJs
                | StackKind::ExpoReactNative
                | StackKind::TypeScript
        )
    });
    if has_js {
        extras.extend(
            ["package.json", "tsconfig.json", "package-lock.json", "pnpm-lock.yaml", "yarn.lock", "bun.lock"]
                .into_iter()
                .map(str::to_owned),
        );
    }
    if config.stacks.contains(&StackKind::Rust) {
        extras.extend(["Cargo.toml", "Cargo.lock", "clippy.toml", "crates", "src"].into_iter().map(str::to_owned));
    }
    if config.stacks.contains(&StackKind::KotlinAndroid) {
        extras.extend(
            ["app", "gradle", "gradlew", "gradlew.bat", "build.gradle.kts", "settings.gradle.kts", "build.gradle", "settings.gradle", "gradle.properties"]
                .into_iter()
                .map(str::to_owned),
        );
    }
    if config.stacks.contains(&StackKind::FlutterDart) {
        extras.extend(["pubspec.yaml", "pubspec.lock", "analysis_options.yaml", "lib", "test"].into_iter().map(str::to_owned));
    }
    if config.stacks.contains(&StackKind::Python) {
        extras.extend(["pyproject.toml", "requirements.txt", "uv.lock", "poetry.lock"].into_iter().map(str::to_owned));
    }
    if config.stacks.contains(&StackKind::Godot) {
        extras.extend(
            ["project.godot", "export_presets.cfg", "default_bus_layout.tres", "icon.svg"]
                .into_iter()
                .map(str::to_owned),
        );
    }
    push_php_laravel_allowlist(&config.stacks, &mut extras);
    for sub in &config.subrepos {
        if let Some(top) = sub.path.split('/').next().filter(|s| !s.is_empty() && *s != ".") {
            extras.push(top.to_owned());
        }
    }
    for item in extras {
        if !config.root_allowlist.iter().any(|existing| existing == &item) {
            config.root_allowlist.push(item);
        }
    }
}

fn push_php_laravel_allowlist(stacks: &[StackKind], extras: &mut Vec<String>) {
    if stacks.contains(&StackKind::Php) || stacks.contains(&StackKind::Laravel) {
        extras.extend(
            [
                "composer.json", "composer.lock", "phpunit.xml", "phpunit.xml.dist",
                "phpstan.neon", "phpstan.neon.dist", "psalm.xml", "psalm.xml.dist",
                "pint.json", ".php-cs-fixer.php", ".php-cs-fixer.dist.php", "src", "tests",
            ]
            .into_iter()
            .map(str::to_owned),
        );
    }
    if stacks.contains(&StackKind::Laravel) {
        extras.extend(
            [
                "artisan", "app", "bootstrap", "config", "database",
                "lang", "public", "resources", "routes", "storage",
            ]
            .into_iter()
            .map(str::to_owned),
        );
    }
}

/// Scans `workspace_root` for project manifests at root and across child repositories.
pub fn detect_workspace(workspace_root: &Path) -> Result<WorkspaceDetection> {
    let mut repos = Vec::new();
    if let Some(root_repo) = inspect_repo_dir(workspace_root, ".")? {
        repos.push(root_repo);
    }

    let ignored_dirs = load_gitignored_dirs(workspace_root);
    collect_child_repos(workspace_root, workspace_root, 0, 2, &ignored_dirs, &mut repos)?;
    repos.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(WorkspaceDetection {
        is_multi_repo: repos.len() > 1 || repos.iter().any(|r| r.rel_path != "."),
        repos,
    })
}

fn collect_child_repos(
    root: &Path,
    dir: &Path,
    depth: usize,
    max_depth: usize,
    gitignored_dirs: &[(String, bool)],
    out: &mut Vec<DetectedRepo>,
) -> Result<()> {
    if depth >= max_depth {
        return Ok(());
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() || is_ignored_dir(&path) {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        if is_gitignored_rel(&rel, gitignored_dirs) {
            continue;
        }
        if let Some(repo) = inspect_repo_dir(&path, &rel)? {
            out.push(repo);
        } else {
            collect_child_repos(root, &path, depth + 1, max_depth, gitignored_dirs, out)?;
        }
    }
    Ok(())
}

fn load_gitignored_dirs(workspace_root: &Path) -> Vec<(String, bool)> {
    let Ok(raw) = fs::read_to_string(workspace_root.join(".gitignore")) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('!') {
                return None;
            }
            let anchored = trimmed.starts_with('/');
            let mut pat = trimmed.trim_start_matches('/').trim_end_matches('/').to_owned();
            if let Some(stripped) = pat.strip_suffix("/**") {
                pat = stripped.trim_end_matches('/').to_owned();
            } else if let Some(stripped) = pat.strip_suffix("/*") {
                pat = stripped.trim_end_matches('/').to_owned();
            }
            if pat.is_empty() || pat.contains(['*', '?', '[']) {
                return None;
            }
            let single_segment = !anchored && !pat.contains('/');
            Some((pat, single_segment))
        })
        .collect()
}

fn is_gitignored_rel(rel: &str, patterns: &[(String, bool)]) -> bool {
    patterns.iter().any(|(pat, single_segment)| {
        if *single_segment {
            rel.split('/').any(|seg| seg == pat)
        } else {
            rel == pat || rel.starts_with(&format!("{pat}/"))
        }
    })
}

fn is_ignored_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return true;
    };
    name.starts_with('.')
        || matches!(
            name,
            "node_modules" | "target" | "dist" | "build" | "venv" | "__pycache__" | "Pods" | "vendor" | "storage"
        )
}

fn inspect_repo_dir(dir: &Path, rel_path: &str) -> Result<Option<DetectedRepo>> {
    let mut stacks = FxHashSet::default();
    let mut tools = FxHashSet::default();

    ecosystems::detect_js_ts_stacks(dir, &mut stacks, &mut tools)?;
    ecosystems::detect_python_stack(dir, &mut stacks, &mut tools)?;
    ecosystems::detect_kotlin_android_stack(dir, &mut stacks, &mut tools)?;
    ecosystems::detect_php_laravel_stack(dir, &mut stacks, &mut tools)?;
    ecosystems::detect_flutter_rust_tools(dir, &mut stacks, &mut tools);

    let has_git = dir.join(".git").exists();
    let has_pkg_json = dir.join("package.json").is_file();
    let is_child = rel_path != ".";
    if stacks.is_empty() && tools.is_empty() && !(is_child && (has_git || has_pkg_json)) {
        return Ok(None);
    }
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("root")
        .to_owned();
    let mut sorted_stacks: Vec<_> = stacks.into_iter().collect();
    sorted_stacks.sort();
    let mut sorted_tools: Vec<_> = tools.into_iter().collect();
    sorted_tools.sort();
    Ok(Some(DetectedRepo {
        name,
        rel_path: rel_path.to_owned(),
        stacks: sorted_stacks,
        tools: sorted_tools,
        has_git,
    }))
}
