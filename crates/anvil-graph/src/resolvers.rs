//! Strategy dispatcher and O(1) lookup helpers for resolving AST edges into `NodeId`s.

pub mod angular;
pub mod css;
pub mod dart;
pub mod godot;
pub mod http_routes;
pub mod kotlin;
pub mod nestjs;
mod path;
pub mod php;
pub mod python;
pub mod rust;
pub(crate) mod rust_cargo;
pub mod tsconfig;

pub use path::{join_normalized, normalize_join_rel};

use std::path::Path;

use anvil_core::traits::{EdgeKind, EdgeRef, Language};
use rustc_hash::FxHashMap;

use crate::model::{CodeGraph, GraphEdgeKind, NodeId};

/// Repository-level resolver metadata loaded from `tsconfig.json`, `pubspec.yaml`, Gradle, Cargo, and Composer.
#[derive(Debug, Clone, Default)]
pub struct ResolutionContext {
    pub tsconfig: tsconfig::TsConfigRules,
    pub dart_packages: FxHashMap<String, String>,
    pub gradle_subprojects: FxHashMap<String, String>,
    pub rust_crates: FxHashMap<String, String>,
    pub php_psr4: Vec<php::Psr4Mapping>,
    pub godot: godot::GodotProjectContext,
}

impl ResolutionContext {
    /// Loads language/framework resolution configs from `repo_root`.
    pub fn load(repo_root: &Path) -> Self {
        Self {
            tsconfig: tsconfig::TsConfigRules::load(repo_root),
            dart_packages: dart::load_dart_packages(repo_root),
            gradle_subprojects: kotlin::load_gradle_subprojects(repo_root),
            rust_crates: rust::load_rust_crates(repo_root),
            php_psr4: php::load_php_psr4(repo_root),
            godot: godot::GodotProjectContext::load(repo_root),
        }
    }
}

/// Intermediate resolved or kept-external edge produced in parallel before `CodeGraph` insertion.
pub struct ResolvedEdge {
    pub source: NodeId,
    pub target: EdgeTarget,
    pub kind: GraphEdgeKind,
    pub line: u32,
}

/// Target of a `ResolvedEdge`: an internal `NodeId` or a non-noisy external string.
pub enum EdgeTarget {
    Node(NodeId),
    External(String),
}

/// Resolves a batch of `EdgeRef`s for a single file using two-pass import-aware matching.
pub fn resolve_file_edges(
    language: Language,
    raw_edges: &[EdgeRef],
    graph: &CodeGraph,
    ctx: &ResolutionContext,
) -> Vec<ResolvedEdge> {
    let mut file_imports: Vec<String> = Vec::new();
    let mut resolved_edges = Vec::with_capacity(raw_edges.len());

    let mut pass1_targets: Vec<Option<NodeId>> = Vec::with_capacity(raw_edges.len());
    for edge in raw_edges {
        let target = if is_pass1_edge(edge.kind) {
            resolve_single_edge(edge, language, &[], graph, ctx)
        } else {
            None
        };
        if let Some(tid) = target
            && let Some(node) = graph.get_node(tid)
            && !node.file.is_empty()
            && !file_imports.contains(&node.file)
        {
            file_imports.push(node.file.clone());
        }
        pass1_targets.push(target);
    }

    for (edge, pass1) in raw_edges.iter().zip(pass1_targets) {
        if edge.kind == EdgeKind::ExposesRoute {
            continue;
        }
        let Some(source) = resolve_source_node_id(edge, graph) else {
            continue;
        };
        if edge.kind == EdgeKind::CallsRoute {
            push_route_call_edges(edge, source, graph, &mut resolved_edges);
            continue;
        }
        let resolved_id =
            pass1.or_else(|| resolve_single_edge(edge, language, &file_imports, graph, ctx));
        let target = match resolved_id {
            Some(tid) => EdgeTarget::Node(tid),
            None => {
                if !should_keep_unresolved(edge) {
                    continue;
                }
                EdgeTarget::External(edge.target_path_or_symbol.clone())
            }
        };
        resolved_edges.push(ResolvedEdge {
            source,
            target,
            kind: GraphEdgeKind::from(edge.kind),
            line: edge.line,
        });
    }
    resolved_edges
}

fn push_route_call_edges(
    edge: &EdgeRef,
    source: NodeId,
    graph: &CodeGraph,
    out: &mut Vec<ResolvedEdge>,
) {
    let targets = http_routes::resolve_calls_route_targets(&edge.target_path_or_symbol, graph);
    if targets.is_empty() {
        out.push(ResolvedEdge {
            source,
            target: EdgeTarget::External(edge.target_path_or_symbol.clone()),
            kind: GraphEdgeKind::CallsRoute,
            line: edge.line,
        });
        return;
    }
    for tid in targets {
        out.push(ResolvedEdge {
            source,
            target: EdgeTarget::Node(tid),
            kind: GraphEdgeKind::CallsRoute,
            line: edge.line,
        });
    }
}

fn should_keep_unresolved(edge: &EdgeRef) -> bool {
    match edge.kind {
        EdgeKind::Imports
        | EdgeKind::ReExports
        | EdgeKind::RendersTemplate
        | EdgeKind::Extends
        | EdgeKind::Implements
        | EdgeKind::Provides
        | EdgeKind::Injects
        | EdgeKind::CallsRoute => true,
        EdgeKind::Calls => {
            let t = edge.target_path_or_symbol.trim();
            !t.is_empty()
                && !t.contains('.')
                && !t.contains("::")
                && t.starts_with(|c: char| c.is_ascii_uppercase())
        }
        EdgeKind::UsesProp | EdgeKind::ExposesRoute => false,
    }
}

fn is_pass1_edge(kind: EdgeKind) -> bool {
    matches!(kind, EdgeKind::Imports | EdgeKind::ReExports | EdgeKind::RendersTemplate)
}

fn resolve_source_node_id(edge: &EdgeRef, graph: &CodeGraph) -> Option<NodeId> {
    if let Some(sym) = &edge.source_symbol {
        let sym_id = CodeGraph::symbol_node_id(&edge.source_file, sym);
        if let Some((nid, _)) = graph.node_by_id(&sym_id) {
            return Some(nid);
        }
    }
    let fid = CodeGraph::file_node_id(&edge.source_file);
    graph.node_by_id(&fid).map(|(nid, _)| nid)
}

/// Resolves a single `EdgeRef` using the language-appropriate strategy stack.
pub fn resolve_single_edge(
    edge: &EdgeRef,
    language: Language,
    file_imports: &[String],
    graph: &CodeGraph,
    ctx: &ResolutionContext,
) -> Option<NodeId> {
    match language {
        Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Jsx => {
            tsconfig::resolve_ts_edge(edge, file_imports, graph, &ctx.tsconfig)
                .or_else(|| angular::resolve_angular_edge(edge, file_imports, graph))
                .or_else(|| nestjs::resolve_nestjs_edge(edge, file_imports, graph))
        }
        Language::Html => angular::resolve_angular_edge(edge, file_imports, graph),
        Language::Css | Language::Scss => css::resolve_css_import(edge, graph),
        Language::Python => python::resolve_python_edge(edge, file_imports, graph),
        Language::Kotlin => {
            kotlin::resolve_kotlin_edge(edge, file_imports, graph, &ctx.gradle_subprojects)
        }
        Language::Dart => dart::resolve_dart_edge(edge, file_imports, graph, &ctx.dart_packages),
        Language::Rust => rust::resolve_rust_edge(edge, file_imports, graph, &ctx.rust_crates),
        Language::Php => php::resolve_php_edge(edge, file_imports, graph, &ctx.php_psr4),
        Language::Blade => php::resolve_blade_edge(edge, graph, &ctx.php_psr4),
        Language::GdScript | Language::GodotScene => {
            godot::resolve_godot_edge(edge, graph, &ctx.godot)
        }
        _ => None,
    }
}

/// Picks the best symbol `NodeId` matching `name`, prioritizing `file_imports` and `source_file`.
pub fn pick_preferred_symbol(
    name: &str,
    source_file: &str,
    file_imports: &[String],
    graph: &CodeGraph,
) -> Option<NodeId> {
    if name.is_empty() {
        return None;
    }
    let candidates = graph.symbol_index.get(name)?;

    for &nid in candidates {
        if let Some(node) = graph.get_node(nid)
            && node.label == name
            && file_imports.iter().any(|imp| imp == &node.file)
        {
            return Some(nid);
        }
    }
    for &nid in candidates {
        if let Some(node) = graph.get_node(nid)
            && node.label == name
            && node.file == source_file
        {
            return Some(nid);
        }
    }
    None
}
