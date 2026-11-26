//! Godot 4 `res://`, `uid://`, `[autoload]`, and `class_name` edge resolution.

use std::fs;
use std::path::Path;

use anvil_core::traits::{EdgeKind, EdgeRef, SymbolKind};
use rustc_hash::FxHashMap;

use crate::model::{CodeGraph, NodeId, NodeKind};
use crate::resolvers::{join_normalized, normalize_join_rel};

/// Godot 4 project metadata discovered from `project.godot` and `*.uid` sidecars.
#[derive(Debug, Clone, Default)]
pub struct GodotProjectContext {
    pub project_dirs: Vec<String>,
    pub autoloads: FxHashMap<String, String>,
    pub uid_map: FxHashMap<String, String>,
}

impl GodotProjectContext {
    /// Scans `repo_root` up to depth 2 for `project.godot`, `[autoload]`s, and `*.uid` files.
    pub fn load(repo_root: &Path) -> Self {
        let mut ctx = Self::default();
        collect_projects_at_depth(repo_root, "", 0, &mut ctx);
        ctx.project_dirs.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        if ctx.project_dirs.iter().any(String::is_empty) {
            scan_uid_dir(repo_root, "", &mut ctx.uid_map);
        } else {
            for rel_dir in &ctx.project_dirs {
                scan_uid_dir(&repo_root.join(rel_dir), rel_dir, &mut ctx.uid_map);
            }
        }
        ctx
    }
}

fn collect_projects_at_depth(
    dir: &Path,
    rel_dir: &str,
    depth: usize,
    ctx: &mut GodotProjectContext,
) {
    let project_file = dir.join("project.godot");
    if project_file.is_file() {
        ctx.project_dirs.push(rel_dir.to_owned());
        parse_autoloads(&project_file, rel_dir, &mut ctx.autoloads);
    }
    if depth >= 2 {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if should_skip_godot_dir(name) {
            continue;
        }
        let sub_rel = join_normalized(rel_dir, name);
        collect_projects_at_depth(&path, &sub_rel, depth + 1, ctx);
    }
}

fn should_skip_godot_dir(name: &str) -> bool {
    matches!(name, ".git" | ".godot" | ".harness" | "node_modules" | "target")
}

fn parse_autoloads(
    project_file: &Path,
    project_dir: &str,
    autoloads: &mut FxHashMap<String, String>,
) {
    let Ok(content) = fs::read_to_string(project_file) else {
        return;
    };
    let mut in_autoload = false;
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            in_autoload = line == "[autoload]";
            continue;
        }
        if !in_autoload {
            continue;
        }
        if let Some((name, raw_val)) = line.split_once('=')
            && let Some(rel) = normalize_autoload_path(project_dir, raw_val)
        {
            let key = name.trim();
            if !key.is_empty() {
                autoloads.insert(key.to_owned(), rel);
            }
        }
    }
}

fn normalize_autoload_path(project_dir: &str, raw_val: &str) -> Option<String> {
    let unquoted = raw_val.trim().trim_matches(|c| c == '"' || c == '\'');
    let stripped = unquoted.strip_prefix('*').unwrap_or(unquoted).trim();
    let res_rel = stripped.strip_prefix("res://")?;
    Some(join_normalized(project_dir, res_rel))
}

fn scan_uid_dir(dir: &Path, rel_dir: &str, uid_map: &mut FxHashMap<String, String>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if path.is_dir() {
            if !should_skip_godot_dir(name) {
                scan_uid_dir(&path, &join_normalized(rel_dir, name), uid_map);
            }
            continue;
        }
        if let Some(asset_name) = name.strip_suffix(".uid")
            && let Some(uid) = read_first_uid_line(&path)
        {
            uid_map.insert(uid, join_normalized(rel_dir, asset_name));
        }
    }
}

fn read_first_uid_line(uid_path: &Path) -> Option<String> {
    let content = fs::read_to_string(uid_path).ok()?;
    let first = content.lines().find(|l| !l.trim().is_empty())?.trim();
    first.starts_with("uid://").then(|| first.to_owned())
}

/// Resolves Godot `res://`, `uid://`, `[autoload]`, and `extends` edges to `NodeId`s.
pub fn resolve_godot_edge(
    edge: &EdgeRef,
    graph: &CodeGraph,
    ctx: &GodotProjectContext,
) -> Option<NodeId> {
    let target = edge.target_path_or_symbol.trim();
    if target.is_empty() {
        return None;
    }
    let project_dir = owning_project_dir(&edge.source_file, &ctx.project_dirs);

    if let Some(res_rel) = target.strip_prefix("res://") {
        let rel = join_normalized(project_dir, res_rel);
        return file_node_id(graph, &rel);
    }
    if target.starts_with("uid://") {
        return ctx.uid_map.get(target).and_then(|rel| file_node_id(graph, rel));
    }
    if let Some(rel) = ctx.autoloads.get(target) {
        return file_node_id(graph, rel);
    }
    if edge.kind == EdgeKind::Extends {
        return resolve_extends_target(target, &edge.source_file, project_dir, graph);
    }
    None
}

fn owning_project_dir<'a>(source_file: &str, project_dirs: &'a [String]) -> &'a str {
    for dir in project_dirs {
        if dir.is_empty() || source_file == dir {
            return dir.as_str();
        }
        if let Some(rest) = source_file.strip_prefix(dir.as_str())
            && rest.starts_with('/')
        {
            return dir.as_str();
        }
    }
    ""
}

fn resolve_extends_target(
    target: &str,
    source_file: &str,
    project_dir: &str,
    graph: &CodeGraph,
) -> Option<NodeId> {
    if let Some(nid) = find_godot_symbol(target, project_dir, graph) {
        return Some(nid);
    }
    let from_project = join_normalized(project_dir, target);
    if let Some(nid) = file_node_id(graph, &from_project) {
        return Some(nid);
    }
    let source_dir = source_file.rsplit_once('/').map_or("", |(d, _)| d);
    let from_rel = normalize_join_rel(source_dir, target);
    file_node_id(graph, &from_rel)
}

fn find_godot_symbol(name: &str, project_dir: &str, graph: &CodeGraph) -> Option<NodeId> {
    let candidates = graph.symbol_index.get(name)?;
    let mut matches: Vec<(NodeId, &crate::model::GraphNode)> = candidates
        .iter()
        .filter_map(|&nid| graph.get_node(nid).map(|n| (nid, n)))
        .filter(|(_, n)| n.kind == NodeKind::Symbol && n.label == name)
        .collect();
    matches.sort_by(|(_, a), (_, b)| {
        let a_in_proj = project_dir.is_empty() || a.file.starts_with(project_dir);
        let b_in_proj = project_dir.is_empty() || b.file.starts_with(project_dir);
        let a_is_cls = a.symbol_kind == Some(SymbolKind::Class);
        let b_is_cls = b.symbol_kind == Some(SymbolKind::Class);
        b_in_proj
            .cmp(&a_in_proj)
            .then_with(|| b_is_cls.cmp(&a_is_cls))
            .then_with(|| b.exported.cmp(&a.exported))
            .then_with(|| a.file.cmp(&b.file))
            .then_with(|| a.line.cmp(&b.line))
    });
    matches.first().map(|(nid, _)| *nid)
}

fn file_node_id(graph: &CodeGraph, rel: &str) -> Option<NodeId> {
    graph.node_by_id(&CodeGraph::file_node_id(rel)).map(|(nid, _)| nid)
}
