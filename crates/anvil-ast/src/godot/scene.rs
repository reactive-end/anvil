//! Pure-Rust Godot scene/resource (`.tscn`, `.tres`) parser and Tier 0 rules.

use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_core::traits::{
    Diagnostic, EdgeKind, EdgeRef, FileAstFacts, Language, Severity, SymbolDef, SymbolKind,
};
use rustc_hash::FxHashMap;

use super::{has_godot_allow, push_diag};

const PHYSICS_BODIES: &[&str] = &[
    "CharacterBody2D",
    "CharacterBody3D",
    "RigidBody2D",
    "RigidBody3D",
    "StaticBody2D",
    "StaticBody3D",
    "Area2D",
    "Area3D",
];

const COLLISION_SHAPES: &[&str] = &[
    "CollisionShape2D",
    "CollisionShape3D",
    "CollisionPolygon2D",
    "CollisionPolygon3D",
];

struct ParsedNode {
    line: u32,
    end_line: u32,
    name: String,
    node_type: Option<String>,
    parent: Option<String>,
    path: String,
    instance: Option<String>,
    script: Option<String>,
}

/// Analyzes a Godot `.tscn` or `.tres` file.
pub fn analyze(
    path: &Path,
    normalized_file: &str,
    source: &str,
    blake3_hash: String,
    config: &AnvilConfig,
) -> FileAstFacts {
    let is_tscn = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("tscn"));
    let lines: Vec<&str> = source
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();
    let comments: Vec<Option<&str>> = lines
        .iter()
        .map(|l| {
            let trimmed = l.trim_start();
            (trimmed.starts_with(';') || trimmed.starts_with('#')).then_some(trimmed)
        })
        .collect();

    let mut ext_resources: FxHashMap<String, (String, String)> = FxHashMap::default();
    let mut edges = Vec::new();
    let mut nodes = Vec::new();

    parse_scene_sections(
        normalized_file,
        &lines,
        &mut ext_resources,
        &mut edges,
        &mut nodes,
    );

    let mut diagnostics = Vec::new();
    if is_tscn {
        check_collision_required(normalized_file, &nodes, &comments, config, &mut diagnostics);
    }

    let symbols = nodes.into_iter().map(|n| node_to_symbol(normalized_file, n)).collect();

    FileAstFacts {
        path: normalized_file.to_owned(),
        blake3_hash,
        language: Language::GodotScene,
        symbols,
        edges,
        diagnostics,
    }
}

fn parse_scene_sections(
    file: &str,
    lines: &[&str],
    ext_resources: &mut FxHashMap<String, (String, String)>,
    edges: &mut Vec<EdgeRef>,
    nodes: &mut Vec<ParsedNode>,
) {
    let mut active_node_idx: Option<usize> = None;

    for (idx, raw) in lines.iter().enumerate() {
        let line_num = idx as u32 + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with(';') || trimmed.starts_with('#') {
            continue;
        }
        if let Some(inside) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            active_node_idx = None;
            if let Some(rest) = inside.strip_prefix("ext_resource")
                && rest.starts_with(char::is_whitespace)
            {
                handle_ext_resource(file, line_num, rest, ext_resources, edges);
            } else if let Some(rest) = inside.strip_prefix("node")
                && rest.starts_with(char::is_whitespace)
                && let Some(node) = parse_node_header(line_num, rest)
            {
                nodes.push(node);
                active_node_idx = Some(nodes.len() - 1);
            }
            continue;
        }

        if let Some(node_idx) = active_node_idx {
            nodes[node_idx].end_line = line_num;
            if let Some(script_val) = parse_script_assignment(trimmed, ext_resources) {
                nodes[node_idx].script = Some(script_val);
            }
        }
    }
}

fn handle_ext_resource(
    file: &str,
    line: u32,
    attrs_raw: &str,
    ext_resources: &mut FxHashMap<String, (String, String)>,
    edges: &mut Vec<EdgeRef>,
) {
    let attrs = parse_header_attrs(attrs_raw);
    let res_type = attrs.get("type").cloned().unwrap_or_default();
    let res_path = attrs.get("path").cloned().unwrap_or_default();
    let res_id = attrs.get("id").cloned().unwrap_or_default();

    if !res_id.is_empty() {
        ext_resources.insert(res_id, (res_type.clone(), res_path.clone()));
    }
    if res_path.is_empty() {
        return;
    }
    let kind = match res_type.as_str() {
        "Script" => EdgeKind::Imports,
        "PackedScene" => EdgeKind::RendersTemplate,
        _ => return,
    };
    edges.push(EdgeRef {
        source_file: file.to_owned(),
        source_symbol: None,
        target_path_or_symbol: res_path,
        kind,
        line,
    });
}

fn parse_node_header(line: u32, attrs_raw: &str) -> Option<ParsedNode> {
    let attrs = parse_header_attrs(attrs_raw);
    let name = attrs.get("name").cloned().filter(|s| !s.is_empty())?;
    let node_type = attrs.get("type").cloned().filter(|s| !s.is_empty());
    let parent = attrs.get("parent").cloned();
    let instance = attrs.get("instance").cloned().filter(|s| !s.is_empty());
    let path = match parent.as_deref() {
        None => ".".to_owned(),
        Some(".") => name.clone(),
        Some(p) => format!("{p}/{name}"),
    };

    Some(ParsedNode {
        line,
        end_line: line,
        name,
        node_type,
        parent,
        path,
        instance,
        script: None,
    })
}

fn parse_script_assignment(
    trimmed: &str,
    ext_resources: &FxHashMap<String, (String, String)>,
) -> Option<String> {
    let (lhs, rhs) = trimmed.split_once('=')?;
    if lhs.trim() != "script" {
        return None;
    }
    let val = rhs.trim();
    if let Some(id) = extract_ext_resource_id(val)
        && let Some((_, path)) = ext_resources.get(id)
        && !path.is_empty()
    {
        return Some(path.clone());
    }
    Some(val.to_owned())
}

fn extract_ext_resource_id(val: &str) -> Option<&str> {
    let inner = val
        .strip_prefix("ExtResource")?
        .trim_start()
        .strip_prefix('(')?
        .strip_suffix(')')?
        .trim();
    Some(inner.trim_matches(|c| c == '"' || c == '\''))
}

fn parse_header_attrs(raw: &str) -> FxHashMap<String, String> {
    let mut map = FxHashMap::default();
    let bytes = raw.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let k_start = i;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() && bytes[i] != b'=' {
            i += 1;
        }
        let key = raw[k_start..i].trim();
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let val = parse_attr_val(raw, &mut i);
        if !key.is_empty() {
            map.insert(key.to_owned(), val);
        }
    }
    map
}

fn parse_attr_val(raw: &str, idx: &mut usize) -> String {
    let bytes = raw.as_bytes();
    if *idx >= bytes.len() {
        return String::new();
    }
    if bytes[*idx] == b'"' {
        *idx += 1;
        let start = *idx;
        while *idx < bytes.len() && bytes[*idx] != b'"' {
            *idx += 1;
        }
        let out = raw[start..*idx].to_owned();
        if *idx < bytes.len() {
            *idx += 1;
        }
        return out;
    }
    let start = *idx;
    let mut paren_depth = 0usize;
    while *idx < bytes.len() {
        match bytes[*idx] {
            b'(' => paren_depth += 1,
            b')' if paren_depth > 0 => paren_depth -= 1,
            b if b.is_ascii_whitespace() && paren_depth == 0 => break,
            _ => {}
        }
        *idx += 1;
    }
    raw[start..*idx].to_owned()
}

fn node_to_symbol(file: &str, node: ParsedNode) -> SymbolDef {
    let mut sig_parts = Vec::new();
    if let Some(ty) = node.node_type {
        sig_parts.push(ty);
    }
    if let Some(script) = node.script {
        sig_parts.push(format!("script={script}"));
    }
    let signature = (!sig_parts.is_empty()).then(|| sig_parts.join(" | "));
    let exported = node.parent.is_none();

    SymbolDef {
        name: node.name,
        qualified_name: node.path,
        kind: SymbolKind::Component,
        file: file.to_owned(),
        line: node.line,
        end_line: node.end_line.max(node.line),
        signature,
        exported,
    }
}

fn check_collision_required(
    file: &str,
    nodes: &[ParsedNode],
    comments: &[Option<&str>],
    config: &AnvilConfig,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for body in nodes {
        if body.instance.is_some() {
            continue;
        }
        let Some(body_type) = body.node_type.as_deref() else {
            continue;
        };
        if !PHYSICS_BODIES.contains(&body_type) {
            continue;
        }
        if has_godot_allow(comments, body.line, "godot4-collision-required", config) {
            continue;
        }
        let has_collision = nodes.iter().any(|child| {
            child.parent.as_deref() == Some(body.path.as_str())
                && child
                    .node_type
                    .as_deref()
                    .is_some_and(|ty| COLLISION_SHAPES.contains(&ty))
        });
        if !has_collision {
            push_diag(
                diagnostics,
                "godot4-collision-required",
                Severity::Error,
                file,
                (body.line, 1),
                format!(
                    "Physics body `{}` ({body_type}) is missing a child CollisionShape/CollisionPolygon node",
                    body.name
                ),
                Some("Add a CollisionShape2D/3D or CollisionPolygon2D/3D child node"),
            );
        }
    }
}
