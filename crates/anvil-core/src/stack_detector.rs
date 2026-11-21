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
}

/// Workspace-level detection summary supporting single-repo and multi-repo roots.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceDetection {
    pub is_multi_repo: bool,
    pub repos: Vec<DetectedRepo>,
}

impl WorkspaceDetection {
    /// Applies detected stacks, tools, and subrepos onto an [`AnvilConfig`].
    pub fn apply_to_config(&self, config: &mut AnvilConfig) {
        let mut all_stacks = FxHashSet::default();
        let mut all_tools = FxHashSet::default();
        config.subrepos.clear();
        for repo in &self.repos {
            all_stacks.extend(repo.stacks.iter().copied());
            all_tools.extend(repo.tools.iter().copied());
            if self.is_multi_repo || repo.rel_path != "." {
                config.subrepos.push(SubrepoConfig {
                    name: repo.name.clone(),
                    path: repo.rel_path.clone(),
                    stacks: repo.stacks.clone(),
                    tools: repo.tools.clone(),
                });
            }
        }
        config.stacks = all_stacks.into_iter().collect();
        config.tools = all_tools.into_iter().collect();
    }
}

/// Scans `workspace_root` for project manifests at root and across child repositories.
pub fn detect_workspace(workspace_root: &Path) -> Result<WorkspaceDetection> {
    let mut repos = Vec::new();
    if let Some(root_repo) = inspect_repo_dir(workspace_root, ".")? {
        repos.push(root_repo);
    }

    collect_child_repos(workspace_root, workspace_root, 0, 2, &mut repos)?;
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
        if let Some(repo) = inspect_repo_dir(&path, &rel)? {
            out.push(repo);
        } else {
            collect_child_repos(root, &path, depth + 1, max_depth, out)?;
        }
    }
    Ok(())
}

fn is_ignored_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return true;
    };
    matches!(
        name,
        ".git"
            | ".harness"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".gradle"
            | ".dart_tool"
            | ".venv"
            | "venv"
    )
}

fn inspect_repo_dir(dir: &Path, rel_path: &str) -> Result<Option<DetectedRepo>> {
    let mut stacks = FxHashSet::default();
    let mut tools = FxHashSet::default();

    ecosystems::detect_js_ts_stacks(dir, &mut stacks, &mut tools)?;
    ecosystems::detect_python_stack(dir, &mut stacks, &mut tools)?;
    ecosystems::detect_kotlin_android_stack(dir, &mut stacks, &mut tools)?;
    ecosystems::detect_flutter_rust_tools(dir, &mut stacks, &mut tools);

    if stacks.is_empty() && tools.is_empty() {
        return Ok(None);
    }
    let name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("root")
        .to_owned();
    Ok(Some(DetectedRepo {
        name,
        rel_path: rel_path.to_owned(),
        stacks: stacks.into_iter().collect(),
        tools: tools.into_iter().collect(),
    }))
}
