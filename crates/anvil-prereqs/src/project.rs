//! The directories a workspace is made of: the root and, in a monorepo, its subrepos.

use std::path::{Path, PathBuf};

use anvil_core::config::prereqs::PrereqSet;
use anyhow::{Result, bail};

/// One project directory. The workspace root has no name; subrepos are named like `SubrepoConfig::name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    pub name: Option<String>,
    /// Path relative to the workspace root, `.` for the root.
    pub rel_path: String,
}

impl Project {
    pub fn root() -> Self {
        Self { name: None, rel_path: ".".to_owned() }
    }

    pub fn subrepo(name: &str, rel_path: &str) -> Self {
        Self { name: Some(name.to_owned()), rel_path: rel_path.to_owned() }
    }

    pub fn dir(&self, root: &Path) -> PathBuf {
        if self.rel_path == "." { root.to_path_buf() } else { root.join(&self.rel_path) }
    }

    /// `rel` as a workspace-relative path with forward slashes (`orders-api/package.json`).
    pub fn rel_file(&self, rel: &str) -> String {
        if self.rel_path == "." { rel.to_owned() } else { format!("{}/{}", self.rel_path, rel) }
    }
}

/// The root plus the given `(name, rel_path)` subrepos.
pub fn projects_with(subrepos: &[(String, String)]) -> Vec<Project> {
    let mut all = vec![Project::root()];
    all.extend(subrepos.iter().filter(|(_, p)| p != ".").map(|(n, p)| Project::subrepo(n, p)));
    all
}

/// Keeps the workspace-level items plus those of the subrepo `name`.
pub fn restrict_to_subrepo(
    projects: &[Project],
    set: &PrereqSet,
    name: &str,
) -> Result<(Vec<Project>, PrereqSet)> {
    if !projects.iter().any(|p| p.name.as_deref() == Some(name)) {
        let known: Vec<&str> = projects.iter().filter_map(|p| p.name.as_deref()).collect();
        bail!(
            "unknown subrepo `{name}` (known: {})",
            if known.is_empty() { "none".into() } else { known.join(", ") }
        );
    }
    let keep = |sub: &Option<String>| sub.as_deref().is_none_or(|s| s == name);
    let mut out = set.clone();
    out.tools.retain(|t| keep(&t.subrepo));
    out.env_files.retain(|e| keep(&e.subrepo));
    out.env_vars.retain(|e| keep(&e.subrepo));
    out.paths.retain(|p| keep(&p.subrepo));
    out.custom.retain(|c| keep(&c.subrepo));
    let projects =
        projects.iter().filter(|p| p.name.as_deref().is_none_or(|n| n == name)).cloned().collect();
    Ok((projects, out))
}
