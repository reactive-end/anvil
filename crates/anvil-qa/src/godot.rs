//! Godot 4 Movie Maker runner command builder and `.tscn` SceneTree QA audit.

use std::path::Path;

use crate::model::{QaViolation, QaViolationKind, RunnerCommand};

const SCENE_3D_TYPES: &[&str] =
    &["Node3D", "CharacterBody3D", "RigidBody3D", "StaticBody3D", "MeshInstance3D", "Camera3D"];

const LIGHTING_3D_TYPES: &[&str] =
    &["DirectionalLight3D", "OmniLight3D", "SpotLight3D", "WorldEnvironment"];

const PHYSICS_BODY_TYPES: &[&str] = &[
    "CharacterBody2D",
    "CharacterBody3D",
    "RigidBody2D",
    "RigidBody3D",
    "StaticBody2D",
    "StaticBody3D",
    "Area2D",
    "Area3D",
];

const COLLISION_SHAPE_TYPES: &[&str] =
    &["CollisionShape2D", "CollisionShape3D", "CollisionPolygon2D", "CollisionPolygon3D"];

#[derive(Debug, Clone)]
struct SceneNode {
    name: String,
    node_type: Option<String>,
    parent: Option<String>,
    path: String,
}

/// Builds a Godot 4 Movie Maker command to render frames to PNG without `--headless`
/// so the GPU renders visual frames for inspection.
pub fn build_godot_movie_command(
    godot_bin: &Path,
    project_dir: &Path,
    out_png: &Path,
    frames: u32,
) -> RunnerCommand {
    RunnerCommand {
        program: godot_bin.to_string_lossy().into_owned(),
        args: vec![
            "--path".to_owned(),
            project_dir.to_string_lossy().into_owned(),
            "--write-movie".to_owned(),
            out_png.to_string_lossy().into_owned(),
            "--fixed-fps".to_owned(),
            "30".to_owned(),
            "--quit-after".to_owned(),
            frames.max(1).to_string(),
        ],
    }
}

/// Audits parsed `[node ...]` headers in a Godot `.tscn` scene for missing 3D camera,
/// missing lighting/environment, and physics bodies lacking collision shapes.
pub fn audit_godot_scene_tree(tscn_source: &str, file: &str) -> Vec<QaViolation> {
    let nodes = parse_scene_nodes(tscn_source);
    let mut violations = Vec::new();
    check_3d_scene_requirements(&nodes, file, &mut violations);
    check_physics_collisions(&nodes, file, &mut violations);
    violations
}

fn check_3d_scene_requirements(nodes: &[SceneNode], file: &str, violations: &mut Vec<QaViolation>) {
    let is_3d =
        nodes.iter().any(|n| n.node_type.as_deref().is_some_and(|ty| SCENE_3D_TYPES.contains(&ty)));
    if !is_3d {
        return;
    }

    let has_camera = nodes.iter().any(|n| n.node_type.as_deref() == Some("Camera3D"));
    if !has_camera {
        violations.push(QaViolation {
            kind: QaViolationKind::Godot3dMissingCamera,
            selector: file.to_owned(),
            message: "3D scene is missing a Camera3D node (godot-3d-missing-camera)".to_owned(),
            rect: None,
            viewport: Some("godot".to_owned()),
        });
    }

    let has_lighting = nodes
        .iter()
        .any(|n| n.node_type.as_deref().is_some_and(|ty| LIGHTING_3D_TYPES.contains(&ty)));
    if !has_lighting {
        violations.push(QaViolation {
            kind: QaViolationKind::Godot3dMissingLighting,
            selector: file.to_owned(),
            message: "3D scene is missing a lighting or WorldEnvironment node (godot-3d-missing-lighting)".to_owned(),
            rect: None,
            viewport: Some("godot".to_owned()),
        });
    }
}

fn check_physics_collisions(nodes: &[SceneNode], file: &str, violations: &mut Vec<QaViolation>) {
    for body in nodes {
        let Some(body_type) = body.node_type.as_deref() else {
            continue;
        };
        if !PHYSICS_BODY_TYPES.contains(&body_type) {
            continue;
        }
        let has_shape = nodes.iter().any(|child| {
            child.parent.as_deref() == Some(body.path.as_str())
                && child.node_type.as_deref().is_some_and(|ty| COLLISION_SHAPE_TYPES.contains(&ty))
        });
        if !has_shape {
            violations.push(QaViolation {
                kind: QaViolationKind::GodotMissingCollisionShape,
                selector: format!("{file}#{}", body.path),
                message: format!(
                    "physics body `{}` ({body_type}) is missing a child CollisionShape/CollisionPolygon (godot-missing-collision-shape)",
                    body.name
                ),
                rect: None,
                viewport: Some("godot".to_owned()),
            });
        }
    }
}

fn parse_scene_nodes(tscn_source: &str) -> Vec<SceneNode> {
    let mut nodes = Vec::new();
    for raw in tscn_source.lines() {
        let trimmed = raw.trim();
        let Some(inside) = trimmed.strip_prefix('[').and_then(|s| s.strip_suffix(']')) else {
            continue;
        };
        let Some(rest) = inside.strip_prefix("node") else {
            continue;
        };
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        if let Some(node) = parse_node_header(rest) {
            nodes.push(node);
        }
    }
    nodes
}

fn parse_node_header(attrs_raw: &str) -> Option<SceneNode> {
    let attrs = parse_attrs(attrs_raw);
    let name = find_attr(&attrs, "name")?.to_owned();
    if name.is_empty() {
        return None;
    }
    let node_type = find_attr(&attrs, "type").filter(|v| !v.is_empty()).map(str::to_owned);
    let parent = find_attr(&attrs, "parent").map(str::to_owned);
    let path = match parent.as_deref() {
        None => ".".to_owned(),
        Some(".") => name.clone(),
        Some(p) => format!("{p}/{name}"),
    };
    Some(SceneNode { name, node_type, parent, path })
}

fn find_attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn parse_attrs(raw: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let bytes = raw.as_bytes();
    let mut i = 0usize;

    while i < bytes.len() {
        skip_ws(bytes, &mut i);
        let key = read_key(raw, bytes, &mut i);
        skip_ws(bytes, &mut i);
        if i >= bytes.len() || bytes[i] != b'=' {
            if i < bytes.len() {
                i += 1;
            }
            continue;
        }
        i += 1;
        skip_ws(bytes, &mut i);
        let val = read_attr_value(raw, bytes, &mut i);
        if !key.is_empty() {
            out.push((key.to_owned(), val));
        }
    }
    out
}

fn skip_ws(bytes: &[u8], idx: &mut usize) {
    while *idx < bytes.len() && bytes[*idx].is_ascii_whitespace() {
        *idx += 1;
    }
}

fn read_key<'a>(raw: &'a str, bytes: &[u8], idx: &mut usize) -> &'a str {
    let start = *idx;
    while *idx < bytes.len() && !bytes[*idx].is_ascii_whitespace() && bytes[*idx] != b'=' {
        *idx += 1;
    }
    raw[start..*idx].trim()
}

fn read_attr_value(raw: &str, bytes: &[u8], idx: &mut usize) -> String {
    if *idx >= bytes.len() {
        return String::new();
    }
    if bytes[*idx] == b'"' {
        *idx += 1;
        let start = *idx;
        while *idx < bytes.len() && bytes[*idx] != b'"' {
            *idx += 1;
        }
        let val = raw[start..*idx].to_owned();
        if *idx < bytes.len() {
            *idx += 1;
        }
        return val;
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
