//! Incremental file removal and `u32` slot tombstoning helpers for `builder`.

use rustc_hash::FxHashMap;

use crate::model::{CodeGraph, GraphEdgeKind, GraphNode, INVALID_NODE_ID, NodeId, NodeKind};

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
        let Some(rev_slot) = graph.reverse_adj.get_mut(nid as usize) else {
            continue;
        };
        let in_idxs = std::mem::take(rev_slot);
        let mut retarget_edges = Vec::new();
        for e_idx in in_idxs {
            if let Some(edge) = graph.edges.get_mut(e_idx as usize)
                && !edge.is_tombstone()
            {
                if edge.kind == GraphEdgeKind::ExposesRoute {
                    edge.source = INVALID_NODE_ID;
                    graph.tombstone_edges += 1;
                } else {
                    retarget_edges.push(e_idx);
                }
            }
        }
        if !retarget_edges.is_empty() {
            let ext = graph.intern_external(&old_id);
            for e_idx in retarget_edges {
                graph.edges[e_idx as usize].target = ext;
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
