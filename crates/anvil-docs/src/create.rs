//! Creating bundles and concepts, and keeping `index.md` and `log.md` current.

use std::fs;
use std::path::{Path, PathBuf};

use anvil_core::fs_util::atomic_write;
use anvil_graph::model::CodeGraph;
use anyhow::{Context, Result, bail};

use crate::bundle::Bundle;
use crate::frontmatter::Document;
use crate::markdown_edit::{add_log_entry, append_to_section};
use crate::meta::Stamp;
use crate::paths::PathMatcher;
use crate::yaml::{self, Mapping, Yaml};
use crate::{
    KEY_PATHS, KEY_SYNCED_COMMIT, KEY_SYNCED_DIGEST, OKF_VERSION, drift, now_rfc3339, outline,
    producer, today,
};

const MODULES_HEADING: &str = "## Modules";

/// What to write for a new module concept.
#[derive(Debug, Clone, Default)]
pub struct NewConcept {
    /// Module path inside the bundle, e.g. `orders` or `anvil-core/config`.
    pub module: String,
    /// Repo-relative globs of the code the concept documents.
    pub paths: Vec<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    /// Bullets for the Responsibilities section (for example the layers of a module).
    pub responsibilities: Vec<String>,
}

/// Validates a module path and returns it normalised (forward slashes, no empty segments).
pub fn normalize_module(module: &str) -> Result<String> {
    let cleaned = module.trim().replace('\\', "/");
    let segments: Vec<&str> = cleaned.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        bail!("module name is empty");
    }
    for seg in &segments {
        let ok = seg.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if !ok || seg.starts_with('.') {
            bail!("invalid module segment `{seg}` (use letters, digits, `-`, `_`, `.`)");
        }
    }
    Ok(segments.join("/"))
}

/// Writes `text` unless the file exists; returns whether it was created.
fn write_new(path: &Path, text: &str) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    atomic_write(path, text.as_bytes())?;
    Ok(true)
}

/// Creates the bundle directory with its `index.md` and `log.md`; existing files are kept. Returns
/// the files that were created.
pub fn init_bundle(bundle: &Bundle) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(&bundle.dir)
        .with_context(|| format!("cannot create {}", bundle.dir.display()))?;
    let project = &bundle.project;
    let index = format!(
        "---\nokf_version: \"{OKF_VERSION}\"\ntitle: {project} documentation\ndescription: Living documentation of {project}, kept in sync with the code by Anvil.\n---\n\n# {project}\n\nRead this index first, then the concept of the module you are about to change.\n\n{MODULES_HEADING}\n"
    );
    let log = "# Log\n\nChange history of this bundle, newest first.\n";
    let mut created = Vec::new();
    if write_new(&bundle.index_path(), &index)? {
        created.push(bundle.index_path());
    }
    if write_new(&bundle.log_path(), log)? {
        created.push(bundle.log_path());
        append_log(bundle, "Initialised the bundle.")?;
    }
    Ok(created)
}

/// Adds a dated entry to the bundle `log.md`.
pub fn append_log(bundle: &Bundle, entry: &str) -> Result<()> {
    let path = bundle.log_path();
    let current = fs::read_to_string(&path).unwrap_or_else(|_| "# Log\n".to_owned());
    let updated = add_log_entry(&current, &today(), entry);
    atomic_write(&path, updated.as_bytes())
}

/// Adds the concept to the bundle `index.md` unless it is already linked.
pub fn add_to_index(bundle: &Bundle, link_path: &str, label: &str, summary: &str) -> Result<()> {
    let path = bundle.index_path();
    let current = fs::read_to_string(&path).unwrap_or_default();
    if current.contains(&format!("]({link_path})")) {
        return Ok(());
    }
    let line = format!("- [{label}]({link_path}) - {summary}");
    let updated = append_to_section(&current, MODULES_HEADING, &line);
    atomic_write(&path, updated.as_bytes())
}

fn frontmatter(spec: &NewConcept, title: &str, description: &str, root: &Path) -> Mapping {
    let mut meta = Mapping::new();
    let mut put = |key: &str, value: Yaml| yaml::set(&mut meta, key, value);
    put("type", yaml::string("Code Module"));
    put("title", yaml::string(title));
    put("description", yaml::string(description));
    put("status", yaml::string("draft"));
    put("generated", Stamp { by: producer(), at: now_rfc3339() }.to_value());
    put(KEY_PATHS, Yaml::Array(spec.paths.iter().map(yaml::string).collect()));
    let (commit, digest) = drift::sync_state(root, &spec.paths);
    if let Some(commit) = commit {
        put(KEY_SYNCED_COMMIT, yaml::string(commit));
    }
    if let Some(digest) = digest {
        put(KEY_SYNCED_DIGEST, yaml::string(digest));
    }
    meta
}

fn body(spec: &NewConcept, title: &str, outline_inner: &str) -> String {
    let responsibilities = if spec.responsibilities.is_empty() {
        "_What this module is responsible for, and what it is not._\n".to_owned()
    } else {
        spec.responsibilities.iter().map(|r| format!("- {r}\n")).collect()
    };
    let purpose = spec
        .description
        .as_deref()
        .map_or_else(|| "_Why this module exists, in a few sentences._".to_owned(), str::to_owned);
    let block = outline::block(outline_inner);
    format!(
        "# {title}\n\n## Purpose\n\n{purpose}\n\n## Responsibilities\n\n{responsibilities}\n## Public interface\n\n{block}\n\n## Invariants\n\n_What must always hold, and what callers rely on._\n\n## Dependencies\n\n_What this module needs from the rest of the system, and why._\n\n## Related\n\n_Neighbouring modules and concepts, as links._\n"
    )
}

/// Creates `<module>/overview.md` in `bundle`, registers it in `index.md`, and logs it. Fails when the
/// concept already exists. Returns the absolute path of the new file.
pub fn create_concept(
    root: &Path,
    bundle: &Bundle,
    spec: &NewConcept,
    graph: Option<&CodeGraph>,
) -> Result<PathBuf> {
    let module = normalize_module(&spec.module)?;
    if spec.paths.is_empty() {
        bail!("at least one code path glob is required (--paths)");
    }
    let matcher = PathMatcher::new(&spec.paths)?;
    let file = bundle.dir.join(&module).join("overview.md");
    if file.exists() {
        bail!("concept `{}/{module}` already exists ({})", bundle.project, file.display());
    }
    init_bundle(bundle)?;
    let leaf = module.rsplit('/').next().unwrap_or(&module);
    let title = spec.title.clone().unwrap_or_else(|| leaf.to_owned());
    let description =
        spec.description.clone().unwrap_or_else(|| format!("Overview of the `{module}` module."));
    let inner =
        graph.map_or_else(|| outline::NO_GRAPH_HINT.to_owned(), |g| outline::render(g, &matcher));
    let doc =
        Document::new(frontmatter(spec, &title, &description, root), body(spec, &title, &inner));
    atomic_write(&file, doc.render().as_bytes())?;
    let link = format!("{module}/overview.md");
    add_to_index(bundle, &link, &module, &description)?;
    append_log(bundle, &format!("Created [{module}]({link})."))?;
    Ok(file)
}
