//! Per-file AST/index mutation helpers.

use std::fs;
use std::path::Path;

use anyhow::Result;
use anvil_ast::analyze_file;
use anvil_core::config::AnvilConfig;
use anvil_core::traits::{EdgeRef, FileAstFacts, Language};

use crate::model::{CodeGraph, GraphEdge, GraphEdgeKind, GraphNode, NodeKind};
use crate::resolvers::kotlin;

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

pub(crate) struct ExtractedFile {
    pub facts: FileAstFacts,
    pub kotlin_pkg: Option<String>,
}

pub(crate) fn extract_one(
    abs: &Path,
    rel: &str,
    config: &AnvilConfig,
) -> Result<Option<ExtractedFile>> {
    let meta = fs::metadata(abs)?;
    if meta.len() > MAX_FILE_BYTES {
        return Ok(None);
    }
    let bytes = fs::read(abs)?;
    let Ok(source) = String::from_utf8(bytes) else {
        return Ok(None);
    };
    let facts = analyze_file(Path::new(rel), &source, config)?;
    let kotlin_pkg = (facts.language == Language::Kotlin)
        .then(|| kotlin::extract_kotlin_package(&source))
        .flatten();
    Ok(Some(ExtractedFile { facts, kotlin_pkg }))
}

pub(crate) fn insert_file_facts(
    graph: &mut CodeGraph,
    item: ExtractedFile,
    update_live_indexes: bool,
) -> Vec<EdgeRef> {
    let FileAstFacts {
        path,
        blake3_hash,
        language,
        symbols,
        edges,
        diagnostics: _,
    } = item.facts;

    graph.file_hashes.insert(path.clone(), blake3_hash);
    let file_id = CodeGraph::file_node_id(&path);
    let label = path.rsplit('/').next().unwrap_or(&path).to_owned();
    let pkg_sig = item.kotlin_pkg.as_ref().map(|pkg| format!("pkg:{pkg}"));

    graph.nodes.insert(
        file_id.clone(),
        GraphNode {
            id: file_id.clone(),
            kind: NodeKind::File,
            label,
            file: path.clone(),
            line: 1,
            language: Some(language),
            symbol_kind: None,
            signature: pkg_sig,
            exported: true,
            community: None,
        },
    );

    if update_live_indexes {
        graph.file_nodes.entry(path.clone()).or_default().push(file_id.clone());
        if let Some(pkg) = item.kotlin_pkg {
            graph.file_packages.insert(path.clone(), pkg.clone());
            graph.kotlin_packages.entry(pkg).or_default().push(path.clone());
        }
    }

    for sym in symbols {
        insert_symbol_into_graph(graph, &path, &file_id, language, sym, update_live_indexes);
    }
    edges
}

fn insert_symbol_into_graph(
    graph: &mut CodeGraph,
    path: &str,
    file_id: &str,
    language: Language,
    sym: anvil_core::traits::SymbolDef,
    update_live_indexes: bool,
) {
    let sym_id = CodeGraph::symbol_node_id(path, &sym.qualified_name);
    let sym_label = sym.name;
    let is_angular_sel = sym.signature.as_deref() == Some("angular_selector");
    let actual_id = graph.insert_symbol_node(GraphNode {
        id: sym_id,
        kind: NodeKind::Symbol,
        label: sym_label.clone(),
        file: path.to_owned(),
        line: sym.line,
        language: Some(language),
        symbol_kind: Some(sym.kind),
        signature: sym.signature,
        exported: sym.exported,
        community: None,
    });
    if update_live_indexes {
        graph.file_nodes.entry(path.to_owned()).or_default().push(actual_id.clone());
        graph.symbol_index.entry(sym_label.clone()).or_default().push(actual_id.clone());
        let lower = sym_label.to_ascii_lowercase();
        if lower != sym_label {
            graph.symbol_index.entry(lower).or_default().push(actual_id.clone());
        }
        if is_angular_sel {
            graph.angular_selectors.insert(sym_label, actual_id.clone());
        }
    }
    push_edge(
        graph,
        GraphEdge {
            source: file_id.to_owned(),
            target: actual_id.clone(),
            kind: GraphEdgeKind::Contains,
            line: sym.line,
            resolved: true,
            raw_target: actual_id,
        },
        update_live_indexes,
    );
}

pub(crate) fn push_edge(graph: &mut CodeGraph, edge: GraphEdge, update_adj: bool) {
    let idx = graph.edges.len();
    if update_adj {
        graph.forward_adj.entry(edge.source.clone()).or_default().push(idx);
        graph.reverse_adj.entry(edge.target.clone()).or_default().push(idx);
    }
    graph.edges.push(edge);
}

pub(crate) fn remove_file_incremental(graph: &mut CodeGraph, rel: &str) -> bool {
    let had_hash = graph.file_hashes.remove(rel).is_some();
    let Some(removed_ids) = graph.file_nodes.remove(rel) else {
        return had_hash;
    };
    if let Some(pkg) = graph.file_packages.remove(rel)
        && let Some(files) = graph.kotlin_packages.get_mut(&pkg)
    {
        files.retain(|f| f != rel);
    }

    for id in &removed_ids {
        if let Some(node) = graph.nodes.remove(id) {
            remove_from_symbol_index(graph, id, &node);
        }
        if let Some(out_idxs) = graph.forward_adj.remove(id) {
            for idx in out_idxs {
                if let Some(edge) = graph.edges.get_mut(idx) {
                    if let Some(rev) = graph.reverse_adj.get_mut(&edge.target) {
                        rev.retain(|&i| i != idx);
                    }
                    edge.source.clear();
                    edge.resolved = false;
                    graph.tombstone_edges += 1;
                }
            }
        }
        if let Some(in_idxs) = graph.reverse_adj.get(id) {
            for &idx in in_idxs {
                if let Some(edge) = graph.edges.get_mut(idx) {
                    edge.resolved = false;
                }
            }
        }
    }
    true
}

fn remove_from_symbol_index(graph: &mut CodeGraph, id: &str, node: &GraphNode) {
    if node.kind != NodeKind::Symbol {
        return;
    }
    if let Some(vec) = graph.symbol_index.get_mut(&node.label) {
        vec.retain(|x| x != id);
    }
    let lower = node.label.to_ascii_lowercase();
    if lower != node.label
        && let Some(vec) = graph.symbol_index.get_mut(&lower)
    {
        vec.retain(|x| x != id);
    }
    if node.signature.as_deref() == Some("angular_selector") {
        graph.angular_selectors.remove(&node.label);
    }
}

pub(crate) fn reconcile_incoming_for_file(graph: &mut CodeGraph, rel: &str) {
    let Some(new_ids) = graph.file_nodes.get(rel).cloned() else {
        return;
    };
    for id in new_ids {
        if let Some(in_idxs) = graph.reverse_adj.get(&id) {
            for &idx in in_idxs {
                if let Some(edge) = graph.edges.get_mut(idx)
                    && !edge.source.is_empty()
                {
                    edge.resolved = true;
                }
            }
        }
    }
}
