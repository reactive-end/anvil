//! Per-file AST/index mutation helpers with `u32` slot reuse.

use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::Result;
use anvil_ast::analyze_file;
use anvil_core::config::AnvilConfig;
use anvil_core::traits::{EdgeRef, FileAstFacts, Language};
use rustc_hash::FxHashMap;

use crate::model::{
    CodeGraph, CompactEdge, GraphEdgeKind, GraphNode, INVALID_NODE_ID, NodeId, NodeKind,
};
use crate::resolvers::{EdgeTarget, ResolvedEdge, kotlin};

pub(crate) const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

pub(crate) struct ExtractedFile {
    pub facts: FileAstFacts,
    pub kotlin_pkg: Option<String>,
    pub stat: (u64, u64),
}

pub(crate) fn file_stat_from_meta(meta: &fs::Metadata) -> (u64, u64) {
    let len = meta.len();
    let mtime_nanos = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| u64::try_from(d.as_nanos()).unwrap_or(u64::MAX));
    (mtime_nanos, len)
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
    let stat = file_stat_from_meta(&meta);
    let bytes = fs::read(abs)?;
    extract_from_bytes(rel, bytes, stat, config)
}

pub(crate) fn extract_from_bytes(
    rel: &str,
    bytes: Vec<u8>,
    stat: (u64, u64),
    config: &AnvilConfig,
) -> Result<Option<ExtractedFile>> {
    let Ok(source) = String::from_utf8(bytes) else {
        return Ok(None);
    };
    let facts = analyze_file(Path::new(rel), &source, config)?;
    let kotlin_pkg = (facts.language == Language::Kotlin)
        .then(|| kotlin::extract_kotlin_package(&source))
        .flatten();
    Ok(Some(ExtractedFile {
        facts,
        kotlin_pkg,
        stat,
    }))
}

pub(crate) fn insert_file_facts(
    graph: &mut CodeGraph,
    item: ExtractedFile,
    update_live_indexes: bool,
    mut reuse_slots: Option<&mut FxHashMap<String, NodeId>>,
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
    graph.file_stats.insert(path.clone(), item.stat);
    let file_id = CodeGraph::file_node_id(&path);
    let label = path.rsplit('/').next().unwrap_or(&path).to_owned();
    let pkg_sig = item.kotlin_pkg.as_ref().map(|pkg| format!("pkg:{pkg}"));

    let file_reuse = reuse_slots.as_deref_mut().and_then(|m| m.remove(&file_id));
    let file_nid = graph.insert_node_with_slot(
        GraphNode {
            id: file_id,
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
        file_reuse,
    );

    if update_live_indexes {
        graph.file_nodes.entry(path.clone()).or_default().push(file_nid);
        if let Some(pkg) = item.kotlin_pkg {
            graph.file_packages.insert(path.clone(), pkg.clone());
            graph.kotlin_packages.entry(pkg).or_default().push(path.clone());
        }
    }

    for sym in symbols {
        let reuse = reuse_slots.as_deref_mut();
        insert_symbol_into_graph(graph, &path, file_nid, language, sym, update_live_indexes, reuse);
    }
    edges
}

fn insert_symbol_into_graph(
    graph: &mut CodeGraph,
    path: &str,
    file_nid: NodeId,
    language: Language,
    sym: anvil_core::traits::SymbolDef,
    update_live_indexes: bool,
    reuse_slots: Option<&mut FxHashMap<String, NodeId>>,
) {
    let sym_id = CodeGraph::symbol_node_id(path, &sym.qualified_name);
    let reuse = reuse_slots.and_then(|m| m.remove(&sym_id));
    let sym_label = sym.name;
    let is_angular_sel = sym.signature.as_deref() == Some("angular_selector");
    let sym_nid = graph.insert_symbol_node(
        GraphNode {
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
        },
        reuse,
    );
    if update_live_indexes {
        graph.file_nodes.entry(path.to_owned()).or_default().push(sym_nid);
        graph.symbol_index.entry(sym_label).or_default().push(sym_nid);
        if is_angular_sel {
            let label_ref = &graph.nodes[sym_nid as usize].label;
            graph.angular_selectors.insert(label_ref.clone(), sym_nid);
        }
    }
    let edge = CompactEdge { source: file_nid, target: sym_nid, kind: GraphEdgeKind::Contains, line: sym.line };
    graph.push_edge(edge, update_live_indexes);
}

pub(crate) fn push_resolved_edges(
    graph: &mut CodeGraph,
    batch: Vec<ResolvedEdge>,
    update_adj: bool,
) {
    for re in batch {
        let target = match re.target {
            EdgeTarget::Node(tid) => tid,
            EdgeTarget::External(raw) => graph.intern_external(&raw),
        };
        graph.push_edge(
            CompactEdge {
                source: re.source,
                target,
                kind: re.kind,
                line: re.line,
            },
            update_adj,
        );
    }
}

pub(crate) fn remove_file_incremental(
    graph: &mut CodeGraph,
    rel: &str,
) -> (bool, FxHashMap<String, NodeId>) {
    let had_hash = graph.file_hashes.remove(rel).is_some();
    graph.file_stats.remove(rel);
    let mut removed_slots = FxHashMap::default();
    let Some(removed_nids) = graph.file_nodes.remove(rel) else {
        return (had_hash, removed_slots);
    };
    if let Some(pkg) = graph.file_packages.remove(rel)
        && let Some(files) = graph.kotlin_packages.get_mut(&pkg)
    {
        files.retain(|f| f != rel);
    }

    for nid in removed_nids {
        let idx = nid as usize;
        if let Some(node) = graph.nodes.get_mut(idx)
            && !node.is_tombstone()
        {
            if node.kind == NodeKind::Semantic {
                graph.semantic_index.remove(&node.id);
            }
            remove_from_symbol_index(&mut graph.symbol_index, &mut graph.angular_selectors, nid, node);
            let old_id = std::mem::take(&mut node.id);
            node.file.clear();
            node.label.clear();
            graph.tombstone_nodes += 1;
            removed_slots.insert(old_id, nid);
        }
        if let Some(out_slot) = graph.forward_adj.get_mut(idx) {
            let out_idxs = std::mem::take(out_slot);
            for e_idx in out_idxs {
                if let Some(edge) = graph.edges.get_mut(e_idx as usize) {
                    if edge.is_resolved()
                        && let Some(rev) = graph.reverse_adj.get_mut(edge.target as usize)
                    {
                        rev.retain(|&i| i != e_idx);
                    }
                    edge.source = INVALID_NODE_ID;
                    graph.tombstone_edges += 1;
                }
            }
        }
    }
    (true, removed_slots)
}

pub(crate) fn finalize_unused_slots(graph: &mut CodeGraph, unused: FxHashMap<String, NodeId>) {
    for (old_id, nid) in unused {
        let ext = graph.intern_external(&old_id);
        if let Some(rev_slot) = graph.reverse_adj.get_mut(nid as usize) {
            let in_idxs = std::mem::take(rev_slot);
            for e_idx in in_idxs {
                if let Some(edge) = graph.edges.get_mut(e_idx as usize)
                    && !edge.is_tombstone()
                {
                    edge.target = ext;
                }
            }
        }
    }
}

fn remove_from_symbol_index(
    symbol_index: &mut FxHashMap<String, Vec<NodeId>>,
    angular_selectors: &mut FxHashMap<String, NodeId>,
    nid: NodeId,
    node: &GraphNode,
) {
    if node.kind != NodeKind::Symbol {
        return;
    }
    if let Some(vec) = symbol_index.get_mut(&node.label) {
        vec.retain(|&x| x != nid);
    }
    if node.signature.as_deref() == Some("angular_selector") {
        angular_selectors.remove(&node.label);
    }
}
