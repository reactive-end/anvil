//! `Cargo.toml` workspace and crate source-root discovery for Rust resolution.

use std::fs;
use std::path::Path;

use rustc_hash::FxHashMap;

/// Discovers workspace and package crate source roots from `Cargo.toml`.
pub fn load_rust_crates(repo_root: &Path) -> FxHashMap<String, String> {
    let mut map = FxHashMap::default();
    let root_toml = repo_root.join("Cargo.toml");
    let Ok(content) = fs::read_to_string(&root_toml) else {
        return map;
    };

    if let Some(pkg) = parse_package_name(&content) {
        map.insert(pkg.replace('-', "_"), "src".to_owned());
    }

    for member in parse_workspace_members(&content) {
        let norm = member.trim().trim_matches('/');
        if let Some(base) = norm.strip_suffix("/*") {
            let dir = repo_root.join(base);
            if let Ok(entries) = fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    if entry.path().is_dir()
                        && let Some(sub_name) = entry.file_name().to_str()
                    {
                        register_member_crate(repo_root, &format!("{base}/{sub_name}"), &mut map);
                    }
                }
            }
        } else if !norm.is_empty() {
            register_member_crate(repo_root, norm, &mut map);
        }
    }
    map
}

fn register_member_crate(repo_root: &Path, rel_dir: &str, map: &mut FxHashMap<String, String>) {
    let toml_path = repo_root.join(rel_dir).join("Cargo.toml");
    let Ok(content) = fs::read_to_string(toml_path) else {
        return;
    };
    if let Some(pkg) = parse_package_name(&content) {
        let src_dir = if rel_dir == "." {
            "src".to_owned()
        } else {
            format!("{rel_dir}/src")
        };
        map.insert(pkg.replace('-', "_"), src_dir);
    }
}

pub(crate) fn parse_package_name(toml: &str) -> Option<String> {
    let mut in_pkg = false;
    for raw in toml.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_pkg = line == "[package]";
            continue;
        }
        if in_pkg
            && let Some(rest) = line.strip_prefix("name")
            && let Some(val) = rest.trim_start().strip_prefix('=')
        {
            let clean = val.trim().trim_matches(|c| c == '"' || c == '\'');
            if !clean.is_empty() {
                return Some(clean.to_owned());
            }
        }
    }
    None
}

fn parse_workspace_members(toml: &str) -> Vec<String> {
    let mut members = Vec::new();
    let mut in_workspace = false;
    let mut in_members = false;
    for raw in toml.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') && !in_members {
            in_workspace = line == "[workspace]";
            continue;
        }
        if !in_workspace {
            continue;
        }
        if !in_members {
            if let Some(rest) = line.strip_prefix("members")
                && let Some(after_eq) = rest.trim_start().strip_prefix('=')
            {
                in_members = true;
                collect_quoted(after_eq, &mut members);
                if after_eq.contains(']') {
                    in_members = false;
                }
            }
        } else {
            collect_quoted(line, &mut members);
            if line.contains(']') {
                in_members = false;
            }
        }
    }
    members
}

fn collect_quoted(text: &str, out: &mut Vec<String>) {
    let mut parts = text.split('"');
    let _ = parts.next();
    while let Some(val) = parts.next() {
        let trimmed = val.trim();
        if !trimmed.is_empty() {
            out.push(trimmed.to_owned());
        }
        let _ = parts.next();
    }
}
