//! Concepts: the Markdown files of a bundle that declare a `type`.

use std::fs;
use std::path::{Path, PathBuf};

use crate::bundle::Bundle;
use crate::frontmatter::{Document, read_head};

const RESERVED: &[&str] = &["index.md", "log.md"];

/// One documented concept.
#[derive(Debug, Clone)]
pub struct Concept {
    pub project: String,
    /// Path inside the bundle without `.md`, e.g. `orders/overview`.
    pub id: String,
    pub path: PathBuf,
    /// Repo-relative path with forward slashes.
    pub rel_path: String,
    pub doc: Document,
}

impl Concept {
    /// The id users type: `<project>/<module>` for module overviews, else `<project>/<id>`.
    pub fn key(&self) -> String {
        let id = self.id.strip_suffix("/overview").unwrap_or(&self.id);
        format!("{}/{id}", self.project)
    }

    /// Project-qualified OKF id, e.g. `orders-api/orders/overview`.
    pub fn full_id(&self) -> String {
        format!("{}/{}", self.project, self.id)
    }

    /// `true` when `reference` names this concept by key or by full id.
    pub fn is_named(&self, reference: &str) -> bool {
        let wanted = reference.trim().trim_end_matches(".md").trim_matches('/');
        self.key() == wanted || self.full_id() == wanted
    }
}

/// Result of reading every Markdown file of a bundle.
#[derive(Debug, Default)]
pub struct Scan {
    pub concepts: Vec<Concept>,
    /// Files with a frontmatter mapping but no `type`.
    pub untyped: Vec<PathBuf>,
    /// Files whose frontmatter could not be read, with the reason.
    pub broken: Vec<(PathBuf, String)>,
}

/// Finds a concept by key or full id.
pub fn find<'a>(concepts: &'a [Concept], reference: &str) -> Option<&'a Concept> {
    concepts.iter().find(|c| c.is_named(reference))
}

/// Markdown files under `dir`, sorted, skipping hidden directories.
pub fn markdown_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type().is_ok_and(|t| t.is_dir()) {
            if !name.starts_with('.') {
                out.extend(markdown_files(&path));
            }
        } else if name.ends_with(".md") {
            out.push(path);
        }
    }
    out.sort();
    out
}

/// `true` for `index.md` and `log.md`, which are never concepts.
pub fn is_reserved(path: &Path) -> bool {
    path.file_name().is_some_and(|n| RESERVED.contains(&n.to_string_lossy().as_ref()))
}

fn read_document(path: &Path, full: bool) -> Result<Document, String> {
    let text = if full { fs::read_to_string(path) } else { read_head(path) };
    let text = text.map_err(|err| format!("cannot read file: {err}"))?;
    Document::parse(&text).map_err(|err| err.to_string())
}

/// Reads the concepts of `bundle`. With `full == false` only each frontmatter block is read.
pub fn scan(root: &Path, bundle: &Bundle, full: bool) -> Scan {
    let mut result = Scan::default();
    for path in markdown_files(&bundle.dir) {
        if is_reserved(&path) {
            continue;
        }
        match read_document(&path, full) {
            Err(reason) => result.broken.push((path, reason)),
            Ok(doc) if doc.meta().is_none() => {}
            Ok(doc) if doc.concept_type().is_none() => result.untyped.push(path),
            Ok(doc) => result.concepts.push(concept_of(root, bundle, path, doc)),
        }
    }
    result
}

fn concept_of(root: &Path, bundle: &Bundle, path: PathBuf, doc: Document) -> Concept {
    let inside = path.strip_prefix(&bundle.dir).unwrap_or(&path);
    let id = inside.with_extension("").to_string_lossy().replace('\\', "/");
    let rel_path = path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/");
    Concept { project: bundle.project.clone(), id, path, rel_path, doc }
}

/// Reads the concepts of every bundle.
pub fn scan_all(root: &Path, bundles: &[Bundle], full: bool) -> Vec<Concept> {
    bundles.iter().flat_map(|b| scan(root, b, full).concepts).collect()
}
