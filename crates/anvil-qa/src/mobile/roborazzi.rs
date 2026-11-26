//! Roborazzi screenshot diff detection for Kotlin/Compose Android repositories.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::model::{QaViolation, QaViolationKind};

/// Scans standard Roborazzi output directories in Kotlin/Android repos for screenshot diffs.
pub fn scan_roborazzi_diffs(repo_root: &Path) -> Result<Vec<QaViolation>> {
    let mut violations = Vec::new();
    let mut roborazzi_dirs = Vec::new();
    collect_roborazzi_dirs(repo_root, 0, 3, &mut roborazzi_dirs);

    for dir in roborazzi_dirs {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            if name.ends_with("_compare.png")
                || name.ends_with("_diff.png")
                || name.starts_with("diff_")
            {
                let rel = path
                    .strip_prefix(repo_root)
                    .map_or_else(|_| path.display().to_string(), |p| p.display().to_string())
                    .replace('\\', "/");
                violations.push(QaViolation {
                    kind: QaViolationKind::RoborazziDiff,
                    selector: rel,
                    message: format!("Roborazzi visual diff detected: {name}"),
                    rect: None,
                    viewport: Some("roborazzi".to_owned()),
                });
            }
        }
    }
    Ok(violations)
}

fn collect_roborazzi_dirs(dir: &Path, depth: usize, max_depth: usize, out: &mut Vec<PathBuf>) {
    if depth > max_depth {
        return;
    }
    let candidate = dir.join("build/outputs/roborazzi");
    if candidate.is_dir() {
        out.push(candidate);
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if matches!(name, ".git" | ".gradle" | "build" | "node_modules" | "target") {
            continue;
        }
        collect_roborazzi_dirs(&path, depth + 1, max_depth, out);
    }
}
