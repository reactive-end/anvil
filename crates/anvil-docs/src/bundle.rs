//! Bundle discovery: one bundle per project under `.harness/docs/<project>/`.

use std::fs;
use std::path::{Path, PathBuf};

use anvil_core::config::AnvilConfig;
use anyhow::{Result, bail};

/// A directory tree of OKF concepts documenting one project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bundle {
    /// Project name; also the directory name under `.harness/docs/`.
    pub project: String,
    /// Absolute bundle directory.
    pub dir: PathBuf,
    /// Repo-relative directory of the code the project covers (`""` for the whole repository).
    pub code_root: String,
    /// Repo-relative code roots of other bundles inside this one; their files belong to those bundles.
    pub nested: Vec<String>,
}

impl Bundle {
    /// Bundle root `index.md`.
    pub fn index_path(&self) -> PathBuf {
        self.dir.join("index.md")
    }

    /// Bundle root `log.md`.
    pub fn log_path(&self) -> PathBuf {
        self.dir.join("log.md")
    }

    /// Bundle directory relative to the repository, e.g. `.harness/docs/orders-api`.
    pub fn repo_rel(&self) -> String {
        format!(".harness/docs/{}", self.project)
    }
}

/// `.harness/docs/` of a repository.
pub fn docs_dir(root: &Path) -> PathBuf {
    root.join(".harness").join("docs")
}

/// Reduces `name` to lowercase ASCII letters, digits, `-`, and `_`.
pub fn sanitize(name: &str) -> String {
    let mapped: String = name
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c.to_ascii_lowercase() } else { '-' })
        .collect();
    mapped.split('-').filter(|part| !part.is_empty()).collect::<Vec<_>>().join("-")
}

fn default_project(root: &Path, config: &AnvilConfig) -> String {
    if let Some(name) = config.docs.project.as_deref().map(sanitize).filter(|n| !n.is_empty()) {
        return name;
    }
    let folder = root
        .canonicalize()
        .ok()
        .as_deref()
        .and_then(Path::file_name)
        .or_else(|| root.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    Some(sanitize(&folder)).filter(|n| !n.is_empty()).unwrap_or_else(|| "project".to_owned())
}

/// Bundles the configuration calls for, whether or not they exist yet. A monorepo yields one per
/// subrepo; `only` restricts the result to one project name.
pub fn planned(root: &Path, config: &AnvilConfig, only: Option<&str>) -> Result<Vec<Bundle>> {
    let docs = docs_dir(root);
    if config.subrepos.is_empty() {
        let project = only.map_or_else(|| default_project(root, config), sanitize);
        return Ok(vec![Bundle {
            dir: docs.join(&project),
            project,
            code_root: String::new(),
            nested: Vec::new(),
        }]);
    }
    let mut bundles: Vec<Bundle> = config
        .subrepos
        .iter()
        .map(|sub| {
            let project = sanitize(&sub.name);
            let code_root = if sub.path == "." { String::new() } else { clean_rel(&sub.path) };
            Bundle { dir: docs.join(&project), project, code_root, nested: Vec::new() }
        })
        .collect();
    assign_nested(&mut bundles);
    if let Some(name) = only.map(sanitize) {
        bundles.retain(|b| b.project == name);
        if bundles.is_empty() {
            bail!("no subrepo named `{name}` in .harness/config.json");
        }
    }
    Ok(bundles)
}

fn assign_nested(bundles: &mut [Bundle]) {
    let roots: Vec<String> = bundles.iter().map(|b| b.code_root.clone()).collect();
    for bundle in bundles.iter_mut() {
        let own = bundle.code_root.clone();
        bundle.nested = roots
            .iter()
            .filter(|other| !other.is_empty() && **other != own)
            .filter(|other| own.is_empty() || other.starts_with(&format!("{own}/")))
            .cloned()
            .collect();
    }
}

fn clean_rel(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").trim_end_matches('/').to_owned()
}

/// Bundles that exist on disk: every directory directly under `.harness/docs/`.
pub fn existing(root: &Path, config: &AnvilConfig) -> Vec<Bundle> {
    let planned = planned(root, config, None).unwrap_or_default();
    let Ok(entries) = fs::read_dir(docs_dir(root)) else {
        return Vec::new();
    };
    let mut bundles: Vec<Bundle> = entries
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|entry| {
            let project = entry.file_name().to_string_lossy().into_owned();
            if project.starts_with('.') {
                return None;
            }
            let known = planned.iter().find(|b| b.project == project);
            Some(Bundle {
                code_root: known.map(|b| b.code_root.clone()).unwrap_or_default(),
                nested: known.map(|b| b.nested.clone()).unwrap_or_default(),
                project,
                dir: entry.path(),
            })
        })
        .collect();
    bundles.sort_by(|a, b| a.project.cmp(&b.project));
    bundles
}

/// Cheap check used by hooks: does `.harness/docs/` hold at least one bundle directory?
pub fn has_bundles(root: &Path) -> bool {
    fs::read_dir(docs_dir(root)).is_ok_and(|entries| {
        entries.flatten().any(|e| {
            e.file_type().is_ok_and(|t| t.is_dir())
                && !e.file_name().to_string_lossy().starts_with('.')
        })
    })
}

/// The project a repo-relative path under `.harness/docs/` belongs to.
pub fn project_of_doc_path(rel: &str) -> Option<&str> {
    let rest = rel.strip_prefix(".harness/docs/")?;
    let (project, tail) = rest.split_once('/')?;
    (!project.is_empty() && !tail.is_empty()).then_some(project)
}
