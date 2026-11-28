//! Secret hygiene scan: connection strings with embedded passwords in agent configs and helper scripts.
//! Findings are warnings and carry only `file:line` and `scheme://host`.

use std::path::{Path, PathBuf};

use ignore::WalkBuilder;

use super::Ctx;
use crate::creds::find_credentials;
use crate::project::Project;
use crate::report::{Item, Kind, Status};

/// Files larger than this are not scanned.
const MAX_BYTES: u64 = 512 * 1024;
const MAX_FINDINGS: usize = 50;
const FIXED_FILES: &[&str] = &[".mcp.json", ".cursor/mcp.json", ".vscode/mcp.json"];
const JSON_DIRS: &[(&str, &str)] = &[(".claude", "settings"), (".agents", ""), (".mimocode", "")];
const SCRIPT_DIRS: &[&str] = &["scripts", "harness"];
const HINT: &str =
    "Move the password to an environment variable or a secret manager, and rotate it";

pub(super) fn scan(ctx: &Ctx<'_>) -> Vec<Item> {
    let mut items = Vec::new();
    for project in ctx.projects {
        for file in files_of(project, ctx.root) {
            scan_file(ctx.root, project, &file, &mut items);
        }
    }
    if items.len() > MAX_FINDINGS {
        let extra = items.len() - MAX_FINDINGS;
        items.truncate(MAX_FINDINGS);
        items.push(
            Item::new(None, Kind::Secret, "secret-scan", Status::Warning)
                .message(format!("{extra} more connection strings with passwords not listed"))
                .hint(HINT),
        );
    }
    items
}

fn files_of(project: &Project, root: &Path) -> Vec<PathBuf> {
    let dir = project.dir(root);
    let mut files: Vec<PathBuf> =
        FIXED_FILES.iter().map(|f| dir.join(f)).filter(|p| p.is_file()).collect();
    for (sub, prefix) in JSON_DIRS {
        files.extend(json_files(&dir.join(sub), prefix));
    }
    for sub in SCRIPT_DIRS {
        files.extend(walk_files(&dir.join(sub)));
    }
    files
}

fn json_files(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut found: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            p.is_file() && name.starts_with(prefix) && name.ends_with(".json")
        })
        .collect();
    found.sort();
    found
}

fn walk_files(dir: &Path) -> Vec<PathBuf> {
    if !dir.is_dir() {
        return Vec::new();
    }
    let mut found: Vec<PathBuf> = WalkBuilder::new(dir)
        .max_depth(Some(6))
        .build()
        .flatten()
        .map(|e| e.into_path())
        .filter(|p| p.is_file())
        .collect();
    found.sort();
    found
}

fn scan_file(root: &Path, project: &Project, file: &Path, items: &mut Vec<Item>) {
    if std::fs::metadata(file).is_ok_and(|m| m.len() > MAX_BYTES) {
        return;
    }
    let Ok(bytes) = std::fs::read(file) else { return };
    if bytes.contains(&0) {
        return;
    }
    let text = String::from_utf8_lossy(&bytes);
    let shown = file.strip_prefix(root).unwrap_or(file).to_string_lossy().replace('\\', "/");
    for (index, line) in text.lines().enumerate() {
        for cred in find_credentials(line) {
            items.push(
                Item::new(
                    project.name.as_deref(),
                    Kind::Secret,
                    &format!("{shown}:{}", index + 1),
                    Status::Warning,
                )
                .message(format!("connection string with an inline password ({})", cred.label()))
                .hint(HINT),
            );
        }
    }
}
