//! In-memory adjacency, file, symbol, and tombstone-compaction routines for `CodeGraph`.

use crate::model::{CodeGraph, CompactEdge, GraphNode, INVALID_NODE_ID, NodeId, NodeKind};

impl CodeGraph {
    /// Compacts any tombstoned nodes and edges left behind by O(1) incremental updates.
    /// Returns `true` if compaction and full index rebuild ran.
    pub fn compact_tombstones(&mut self) -> bool {
        if self.tombstone_edges == 0 && self.tombstone_nodes == 0 {
            return false;
        }
        if self.tombstone_nodes > 0 {
            let mut remap = vec![INVALID_NODE_ID; self.nodes.len()];
            let mut new_nodes = Vec::with_capacity(self.nodes.len() - self.tombstone_nodes);
            for (old_idx, node) in self.nodes.drain(..).enumerate() {
                if !node.is_tombstone() {
                    let new_idx = new_nodes.len() as u32;
                    remap[old_idx] = new_idx;
                    new_nodes.push(node);
                }
            }
            self.nodes = new_nodes;
            self.tombstone_nodes = 0;

            for edge in &mut self.edges {
                if edge.is_tombstone() {
                    continue;
                }
                let Some(&new_src) = remap.get(edge.source as usize) else {
                    edge.source = INVALID_NODE_ID;
                    continue;
                };
                if new_src == INVALID_NODE_ID {
                    edge.source = INVALID_NODE_ID;
                    continue;
                }
                edge.source = new_src;
                if edge.is_resolved() {
                    let new_tgt =
                        remap.get(edge.target as usize).copied().unwrap_or(INVALID_NODE_ID);
                    edge.target = new_tgt;
                }
            }
        }

        self.edges.retain(|e| !e.is_tombstone());
        self.tombstone_edges = 0;
        self.rebuild_indexes();
        true
    }

    /// Rebuilds all in-memory file, symbol, semantic, and adjacency indexes in O(V + E).
    pub fn rebuild_indexes(&mut self) {
        self.semantic_index.clear();
        self.file_nodes.clear();
        self.symbol_index.clear();
        self.angular_selectors.clear();
        self.kotlin_packages.clear();
        self.file_packages.clear();
        self.external_index.clear();

        self.file_nodes.reserve(self.file_hashes.len());
        self.symbol_index.reserve(self.nodes.len() / 2);

        let mut dead_nodes = 0usize;
        for (idx, node) in self.nodes.iter().enumerate() {
            if node.is_tombstone() {
                dead_nodes += 1;
                continue;
            }
            let nid = idx as u32;
            if !node.file.is_empty() {
                if let Some(list) = self.file_nodes.get_mut(&node.file) {
                    list.push(nid);
                } else {
                    self.file_nodes.insert(node.file.clone(), vec![nid]);
                }
            }
            match node.kind {
                NodeKind::File => {
                    if let Some(sig) = &node.signature
                        && let Some(pkg) = sig.strip_prefix("pkg:")
                    {
                        self.file_packages.insert(node.file.clone(), pkg.to_owned());
                        if let Some(list) = self.kotlin_packages.get_mut(pkg) {
                            list.push(node.file.clone());
                        } else {
                            self.kotlin_packages.insert(pkg.to_owned(), vec![node.file.clone()]);
                        }
                    }
                }
                NodeKind::Symbol => {
                    if let Some(list) = self.symbol_index.get_mut(&node.label) {
                        list.push(nid);
                    } else {
                        self.symbol_index.insert(node.label.clone(), vec![nid]);
                    }
                    if node.signature.as_deref() == Some("angular_selector") {
                        self.angular_selectors.insert(node.label.clone(), nid);
                    }
                }
                NodeKind::Semantic => {
                    self.semantic_index.insert(node.id.clone(), nid);
                }
            }
        }

        self.tombstone_nodes = dead_nodes;
        self.rebuild_adjacency();
    }

    /// Rebuilds ONLY `forward_adj` and `reverse_adj` in two O(V + E) passes with exact capacities.
    pub fn rebuild_adjacency(&mut self) {
        let node_count = self.nodes.len();
        let mut out_deg = vec![0u32; node_count];
        let mut in_deg = vec![0u32; node_count];
        let mut dead_edges = 0usize;

        for edge in &self.edges {
            if edge.is_tombstone() {
                dead_edges += 1;
                continue;
            }
            let src = edge.source as usize;
            if src >= node_count {
                continue;
            }
            out_deg[src] += 1;
            if edge.is_resolved() {
                let tgt = edge.target as usize;
                if tgt < node_count {
                    in_deg[tgt] += 1;
                }
            }
        }
        self.tombstone_edges = dead_edges;

        self.forward_adj.clear();
        self.reverse_adj.clear();
        self.forward_adj.reserve(node_count);
        self.reverse_adj.reserve(node_count);
        for i in 0..node_count {
            self.forward_adj.push(if out_deg[i] == 0 {
                Vec::new()
            } else {
                Vec::with_capacity(out_deg[i] as usize)
            });
            self.reverse_adj.push(if in_deg[i] == 0 {
                Vec::new()
            } else {
                Vec::with_capacity(in_deg[i] as usize)
            });
        }

        for (idx, edge) in self.edges.iter().enumerate() {
            if edge.is_tombstone() {
                continue;
            }
            let src = edge.source as usize;
            if src >= node_count {
                continue;
            }
            let e_idx = idx as u32;
            self.forward_adj[src].push(e_idx);
            if edge.is_resolved() {
                let tgt = edge.target as usize;
                if tgt < node_count {
                    self.reverse_adj[tgt].push(e_idx);
                }
            }
        }
    }

    /// Inserts a symbol node, preserving deterministic IDs on symbol overload collisions.
    pub fn insert_symbol_node(
        &mut self,
        mut node: GraphNode,
        reuse_slot: Option<NodeId>,
    ) -> NodeId {
        if let Some((existing, _)) = self.node_by_id(&node.id)
            && Some(existing) != reuse_slot
        {
            node.id = format!("{}@{}", node.id, node.line);
        }
        self.insert_node_with_slot(node, reuse_slot)
    }

    /// Inserts or reuses `reuse_slot` for `node`, updating `semantic_index` for semantic nodes.
    pub fn insert_node_with_slot(&mut self, node: GraphNode, reuse_slot: Option<NodeId>) -> NodeId {
        let is_semantic = node.kind == NodeKind::Semantic;
        let nid = if let Some(slot) = reuse_slot
            && (slot as usize) < self.nodes.len()
        {
            if self.nodes[slot as usize].is_tombstone() {
                self.tombstone_nodes = self.tombstone_nodes.saturating_sub(1);
            }
            self.nodes[slot as usize] = node;
            slot
        } else {
            let slot = self.nodes.len() as u32;
            self.nodes.push(node);
            self.forward_adj.push(Vec::new());
            self.reverse_adj.push(Vec::new());
            slot
        };
        if is_semantic {
            let id_ref = &self.nodes[nid as usize].id;
            self.semantic_index.insert(id_ref.clone(), nid);
        }
        nid
    }

    /// Pushes a `CompactEdge`, optionally updating `forward_adj` and `reverse_adj` in-place.
    pub fn push_edge(&mut self, edge: CompactEdge, update_adj: bool) {
        let idx = self.edges.len() as u32;
        if update_adj && !edge.is_tombstone() {
            let src = edge.source as usize;
            if src >= self.forward_adj.len() {
                self.forward_adj.resize_with(src + 1, Vec::new);
            }
            self.forward_adj[src].push(idx);
            if edge.is_resolved() {
                let tgt = edge.target as usize;
                if tgt >= self.reverse_adj.len() {
                    self.reverse_adj.resize_with(tgt + 1, Vec::new);
                }
                self.reverse_adj[tgt].push(idx);
            }
        }
        self.edges.push(edge);
    }
}
