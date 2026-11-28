//! The files of a repository as validation and scaffolding see them.

use std::path::Path;

use ignore::WalkBuilder;

/// Lists repo-relative, forward-slash paths of every non-ignored file, sorted. `.git` is skipped.
pub fn list(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();
    for entry in walker.flatten() {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        if let Ok(rel) = entry.path().strip_prefix(root) {
            files.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
    files.sort();
    files
}
