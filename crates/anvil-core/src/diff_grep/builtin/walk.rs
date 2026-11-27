//! Repository walk for the built-in engine: ignore files, hidden policy, noise and size filters.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use ignore::overrides::OverrideBuilder;
use ignore::{DirEntry, WalkBuilder};

use crate::diff_grep::grep::GrepOptions;
use crate::diff_grep::is_noise_file;
use crate::diff_grep::paths::entry_allowed;
use crate::fs_util::relativize_path;

/// Files above this size are skipped (minified bundles, dumps).
pub(super) const MAX_FILE_BYTES: u64 = 512 * 1024;

/// A file selected for searching.
pub(super) struct Candidate {
    pub rel: String,
    pub path: PathBuf,
}

/// Lists searchable files under the requested paths, sorted by repo-relative path.
pub(super) fn collect_files(root: &Path, opts: &GrepOptions) -> Result<Vec<Candidate>> {
    let roots = resolve_roots(root, &opts.paths)?;
    let mut builder = WalkBuilder::new(&roots[0]);
    for extra in &roots[1..] {
        builder.add(extra);
    }
    builder.hidden(false).require_git(false).follow_links(false).max_filesize(Some(MAX_FILE_BYTES));
    if !opts.globs.is_empty() {
        let mut overrides = OverrideBuilder::new(root);
        for glob in &opts.globs {
            overrides.add(glob).with_context(|| format!("invalid glob: {glob}"))?;
        }
        builder.overrides(overrides.build().context("invalid glob set")?);
    }
    let filter_root = root.to_path_buf();
    let include_hidden = opts.hidden;
    builder.filter_entry(move |entry| keep_entry(&filter_root, entry, include_hidden));

    let mut files = Vec::new();
    for entry in builder.build().flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) || !keep_entry(root, &entry, opts.hidden)
        {
            continue;
        }
        let rel = relativize_path(root, entry.path());
        if !is_noise_file(&rel) {
            files.push(Candidate { rel, path: entry.into_path() });
        }
    }
    files.sort_by(|a, b| a.rel.cmp(&b.rel));
    files.dedup_by(|a, b| a.rel == b.rel);
    Ok(files)
}

fn resolve_roots(root: &Path, paths: &[String]) -> Result<Vec<PathBuf>> {
    if paths.is_empty() {
        return Ok(vec![root.to_path_buf()]);
    }
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut roots = Vec::new();
    for raw in paths {
        let candidate = Path::new(raw);
        let full = if candidate.is_absolute() { candidate.to_path_buf() } else { root.join(raw) };
        let Ok(canonical) = full.canonicalize() else {
            bail!("path not found: {raw}");
        };
        if !canonical.starts_with(&canonical_root) {
            bail!("path is outside the repository: {raw}");
        }
        roots.push(full);
    }
    Ok(roots)
}

fn keep_entry(root: &Path, entry: &DirEntry, include_hidden: bool) -> bool {
    if entry.depth() == 0 {
        return true;
    }
    let rel = relativize_path(root, entry.path());
    entry_allowed(&rel, &entry.file_name().to_string_lossy(), include_hidden)
}
