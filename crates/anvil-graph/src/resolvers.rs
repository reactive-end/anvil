//! Strategy dispatcher and O(1) lookup helpers for resolving AST edges into graph node IDs.

pub mod angular;
pub mod dart;
pub mod jsonc;
pub mod kotlin;
pub mod nestjs;
pub mod python;
pub mod tsconfig;

use std::path::Path;

use anvil_core::traits::{EdgeKind, EdgeRef, Language};
use rustc_hash::FxHashMap;

use crate::model::{CodeGraph, GraphEdge, GraphEdgeKind};

/// Repository-level resolver metadata loaded from `tsconfig.json`, `pubspec.yaml`, and Gradle.
#[derive(Debug, Clone, Default)]
pub struct ResolutionContext {
    pub tsconfig: tsconfig::TsConfigRules,
    pub dart_packages: FxHashMap<String, String>,
    pub gradle_subprojects: FxHashMap<String, String>,
}

impl ResolutionContext {
    /// Loads language/framework resolution configs from `repo_root`.
    pub fn load(repo_root: &Path) -> Self {
        Self {
            tsconfig: tsconfig::TsConfigRules::load(repo_root),
            dart_packages: dart::load_dart_packages(repo_root),
            gradle_subprojects: kotlin::load_gradle_subprojects(repo_root),
        }
    }
}

/// Resolves a batch of `EdgeRef`s for a single file using two-pass import-aware matching.
pub fn resolve_file_edges(
    language: Language,
    raw_edges: &[EdgeRef],
    graph: &CodeGraph,
    ctx: &ResolutionContext,
) -> Vec<GraphEdge> {
    let mut file_imports: Vec<String> = Vec::new();
    let mut resolved_edges = Vec::with_capacity(raw_edges.len());

    let mut pass1_targets: Vec<Option<String>> = Vec::with_capacity(raw_edges.len());
    for edge in raw_edges {
        let target = if is_pass1_edge(edge.kind) {
            resolve_single_edge(edge, language, &[], graph, ctx)
        } else {
            None
        };
        if let Some(t) = &target
            && let Some(node) = graph.nodes.get(t)
            && !node.file.is_empty()
            && !file_imports.contains(&node.file)
        {
            file_imports.push(node.file.clone());
        }
        pass1_targets.push(target);
    }

    for (edge, pass1) in raw_edges.iter().zip(pass1_targets) {
        let resolved_id =
            pass1.or_else(|| resolve_single_edge(edge, language, &file_imports, graph, ctx));
        let source_id = resolve_source_node_id(edge, graph);
        let (target, resolved) = match resolved_id {
            Some(id) => (id, true),
            None => (format!("ext:{}", edge.target_path_or_symbol), false),
        };
        resolved_edges.push(GraphEdge {
            source: source_id,
            target,
            kind: GraphEdgeKind::from(edge.kind),
            line: edge.line,
            resolved,
            raw_target: edge.target_path_or_symbol.clone(),
        });
    }
    resolved_edges
}

fn is_pass1_edge(kind: EdgeKind) -> bool {
    matches!(
        kind,
        EdgeKind::Imports | EdgeKind::ReExports | EdgeKind::RendersTemplate
    )
}

fn resolve_source_node_id(edge: &EdgeRef, graph: &CodeGraph) -> String {
    if let Some(sym) = &edge.source_symbol {
        let sym_id = CodeGraph::symbol_node_id(&edge.source_file, sym);
        if graph.nodes.contains_key(&sym_id) {
            return sym_id;
        }
    }
    CodeGraph::file_node_id(&edge.source_file)
}

/// Resolves a single `EdgeRef` using the language-appropriate strategy stack.
pub fn resolve_single_edge(
    edge: &EdgeRef,
    language: Language,
    file_imports: &[String],
    graph: &CodeGraph,
    ctx: &ResolutionContext,
) -> Option<String> {
    match language {
        Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Jsx => {
            tsconfig::resolve_ts_edge(edge, file_imports, graph, &ctx.tsconfig)
                .or_else(|| angular::resolve_angular_edge(edge, file_imports, graph))
                .or_else(|| nestjs::resolve_nestjs_edge(edge, file_imports, graph))
        }
        Language::Html => angular::resolve_angular_edge(edge, file_imports, graph),
        Language::Python => python::resolve_python_edge(edge, file_imports, graph),
        Language::Kotlin => kotlin::resolve_kotlin_edge(edge, file_imports, graph),
        Language::Dart => dart::resolve_dart_edge(edge, file_imports, graph, &ctx.dart_packages),
        _ => None,
    }
}

/// Picks the best symbol node matching `name`, prioritizing `file_imports` and `source_file`.
pub fn pick_preferred_symbol(
    name: &str,
    source_file: &str,
    file_imports: &[String],
    graph: &CodeGraph,
) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    let candidates = graph.symbol_index.get(name)?;

    for id in candidates {
        if let Some(node) = graph.nodes.get(id)
            && node.label == name
            && file_imports.iter().any(|imp| imp == &node.file)
        {
            return Some(id.clone());
        }
    }
    for id in candidates {
        if let Some(node) = graph.nodes.get(id)
            && node.label == name
            && node.file == source_file
        {
            return Some(id.clone());
        }
    }
    None
}

/// Joins a base directory and a relative specifier, resolving `.` and `..` segments cleanly.
pub fn normalize_join_rel(base_dir: &str, rel: &str) -> String {
    let clean_rel = rel.replace('\\', "/");
    let combined = if base_dir.is_empty() || base_dir == "." {
        clean_rel
    } else {
        format!("{base_dir}/{clean_rel}")
    };
    normalize_segments(&combined)
}

/// Joins two normalized relative paths without parent-directory traversal unless present.
pub fn join_normalized(base: &str, sub: &str) -> String {
    let clean_base = base.trim_matches('/');
    let clean_sub = sub.trim_start_matches('/');
    if clean_base.is_empty() || clean_base == "." {
        normalize_segments(clean_sub)
    } else if clean_sub.is_empty() {
        normalize_segments(clean_base)
    } else {
        normalize_segments(&format!("{clean_base}/{clean_sub}"))
    }
}

fn normalize_segments(path: &str) -> String {
    let mut stack: Vec<&str> = Vec::new();
    for seg in path.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            other => stack.push(other),
        }
    }
    stack.join("/")
}
