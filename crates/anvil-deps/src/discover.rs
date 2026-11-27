//! Repository-wide manifest discovery that honours `.gitignore` and skips vendored/build trees.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use crate::audit::is_supported_manifest;

/// Manifests deeper than this (counted from the repo root) are ignored.
const MAX_DEPTH: usize = 5;

const SKIPPED_DIRS: &[&str] =
    &[".git", ".harness", "node_modules", "target", "dist", "build", ".venv", "vendor", "storage"];

/// Finds every supported dependency manifest under `root`, relative to `root` and sorted.
///
/// Directories matched by `.gitignore` (even outside a git checkout) and well-known vendored or build
/// directories are never entered.
pub fn discover_manifests(root: &Path) -> Vec<PathBuf> {
    let walker = WalkBuilder::new(root)
        .hidden(false)
        .parents(false)
        .git_global(false)
        .require_git(false)
        .max_depth(Some(MAX_DEPTH))
        .filter_entry(|entry| {
            let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
            !(is_dir
                && entry.depth() > 0
                && entry.file_name().to_str().is_some_and(|name| SKIPPED_DIRS.contains(&name)))
        })
        .build();

    let mut out: Vec<PathBuf> = walker
        .flatten()
        .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
        .filter(|entry| is_supported_manifest(entry.path()))
        .filter_map(|entry| entry.path().strip_prefix(root).ok().map(Path::to_path_buf))
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use super::*;

    fn touch(root: &Path, rel: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, "{}").expect("write");
    }

    fn rels(found: &[PathBuf]) -> Vec<String> {
        found.iter().map(|p| p.to_string_lossy().replace('\\', "/")).collect()
    }

    #[test]
    fn honours_gitignore_and_skips_vendored_trees() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join(".gitignore"), "/test/\n*.generated/\n").expect("gitignore");
        touch(root, "Cargo.toml");
        touch(root, "app/package.json");
        touch(root, "test/hexagonal-sandbox/orders-api/package.json");
        touch(root, "svc.generated/package.json");
        touch(root, "node_modules/left-pad/package.json");
        touch(root, "target/debug/Cargo.toml");
        touch(root, "vendor/lib/composer.json");

        assert_eq!(rels(&discover_manifests(root)), vec!["Cargo.toml", "app/package.json"]);
    }

    #[test]
    fn nested_gitignore_and_depth_limit_apply() {
        let tmp = TempDir::new().expect("tempdir");
        let root = tmp.path();
        touch(root, "a/b/c/d/e/package.json");
        touch(root, "a/b/c/d/package.json");
        touch(root, "pkg/package.json");
        touch(root, "pkg/legacy/package.json");
        fs::write(root.join("pkg/.gitignore"), "legacy/\n").expect("nested gitignore");

        assert_eq!(
            rels(&discover_manifests(root)),
            vec!["a/b/c/d/package.json", "pkg/package.json"]
        );
    }
}
