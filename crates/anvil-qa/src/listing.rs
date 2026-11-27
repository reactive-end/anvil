//! Finding and naming the JSON files (flows, API scenarios) a QA run works on.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The `*.json` files of `dir`, sorted by name; a missing directory has none.
pub fn json_files(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("failed to read {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|ext| ext == "json") && p.is_file())
        .collect();
    paths.sort();
    Ok(paths)
}

/// The file stem of `path`, which names the flow or scenario on the command line.
pub fn file_id(path: &Path) -> String {
    path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// `true` when `a` and `b` are the same name ignoring case, spaces, and `-`/`_`.
pub fn same_name(a: &str, b: &str) -> bool {
    let key = |text: &str| {
        text.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
    };
    key(a) == key(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_match_loosely_and_missing_dirs_are_empty() {
        assert!(same_name("Create-Order", "create_order"));
        assert!(!same_name("create-order", "create-orders"));
        assert!(json_files(Path::new("definitely/not/here")).expect("list").is_empty());
    }

    #[test]
    fn only_json_files_are_listed_in_name_order() {
        let dir = tempfile::tempdir().expect("tmp");
        for name in ["b.json", "a.json", "notes.md"] {
            fs::write(dir.path().join(name), "{}").expect("write");
        }
        let ids: Vec<String> =
            json_files(dir.path()).expect("list").iter().map(|p| file_id(p)).collect();
        assert_eq!(ids, ["a", "b"]);
    }
}
