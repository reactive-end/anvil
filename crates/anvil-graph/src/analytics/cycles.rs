//! File-level import/re-export cycle detection using iterative Tarjan's SCC.

use std::collections::VecDeque;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::model::{CodeGraph, GraphEdgeKind, NodeKind};

/// Detects import/re-export cycles between files using Tarjan's strongly connected components.
pub fn detect_cycles(graph: &CodeGraph, max_cycles: usize) -> Vec<Vec<String>> {
    let mut files: Vec<String> = graph
        .nodes
        .iter()
        .filter(|n| !n.is_tombstone() && n.kind == NodeKind::File)
        .map(|n| n.file.clone())
        .collect();
    files.sort();
    let idx_map: FxHashMap<&str, usize> =
        files.iter().enumerate().map(|(i, f)| (f.as_str(), i)).collect();

    let mut node_to_file_idx = vec![usize::MAX; graph.nodes.len()];
    for (nid, node) in graph.nodes.iter().enumerate() {
        if !node.is_tombstone()
            && let Some(&fi) = idx_map.get(node.file.as_str())
        {
            node_to_file_idx[nid] = fi;
        }
    }

    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); files.len()];
    for edge in &graph.edges {
        if !edge.is_resolved()
            || !matches!(edge.kind, GraphEdgeKind::Imports | GraphEdgeKind::ReExports)
        {
            continue;
        }
        let u = node_to_file_idx.get(edge.source as usize).copied().unwrap_or(usize::MAX);
        let v = node_to_file_idx.get(edge.target as usize).copied().unwrap_or(usize::MAX);
        if u != usize::MAX && v != usize::MAX && u != v && !adj[u].contains(&v) {
            adj[u].push(v);
        }
    }

    let sccs = tarjan_scc(&adj);
    let mut cycles = Vec::new();
    for scc in sccs {
        if scc.len() < 2 {
            continue;
        }
        if let Some(path) = extract_cycle_in_scc(&adj, &scc, &files) {
            cycles.push(path);
            if cycles.len() >= max_cycles {
                break;
            }
        }
    }
    cycles
}

fn tarjan_scc(adj: &[Vec<usize>]) -> Vec<Vec<usize>> {
    struct Frame {
        u: usize,
        next_edge: usize,
    }
    let n = adj.len();
    let mut index = 0usize;
    let mut indices = vec![usize::MAX; n];
    let mut lowlink = vec![0usize; n];
    let mut on_stack = vec![false; n];
    let mut scc_stack = Vec::new();
    let mut call_stack = Vec::new();
    let mut sccs = Vec::new();

    for start in 0..n {
        if indices[start] != usize::MAX {
            continue;
        }
        indices[start] = index;
        lowlink[start] = index;
        index += 1;
        on_stack[start] = true;
        scc_stack.push(start);
        call_stack.push(Frame { u: start, next_edge: 0 });

        while let Some(top) = call_stack.last_mut() {
            let u = top.u;
            if top.next_edge < adj[u].len() {
                let v = adj[u][top.next_edge];
                top.next_edge += 1;
                if indices[v] == usize::MAX {
                    indices[v] = index;
                    lowlink[v] = index;
                    index += 1;
                    on_stack[v] = true;
                    scc_stack.push(v);
                    call_stack.push(Frame { u: v, next_edge: 0 });
                } else if on_stack[v] {
                    lowlink[u] = lowlink[u].min(indices[v]);
                }
            } else {
                call_stack.pop();
                if let Some(parent) = call_stack.last() {
                    lowlink[parent.u] = lowlink[parent.u].min(lowlink[u]);
                }
                if lowlink[u] == indices[u] {
                    let mut comp = Vec::new();
                    while let Some(w) = scc_stack.pop() {
                        on_stack[w] = false;
                        comp.push(w);
                        if w == u {
                            break;
                        }
                    }
                    sccs.push(comp);
                }
            }
        }
    }
    sccs
}

fn extract_cycle_in_scc(
    adj: &[Vec<usize>],
    scc: &[usize],
    files: &[String],
) -> Option<Vec<String>> {
    let members: FxHashSet<usize> = scc.iter().copied().collect();
    let start = *scc.iter().min()?;
    let mut parent: FxHashMap<usize, usize> = FxHashMap::default();
    let mut queue = VecDeque::new();
    for &next in &adj[start] {
        if members.contains(&next) {
            parent.insert(next, start);
            queue.push_back(next);
        }
    }
    while let Some(u) = queue.pop_front() {
        if u == start {
            let mut cycle = vec![files[start].clone()];
            let mut cur = parent[&start];
            let mut rev = Vec::new();
            while cur != start {
                rev.push(files[cur].clone());
                cur = parent[&cur];
            }
            rev.reverse();
            cycle.extend(rev);
            cycle.push(files[start].clone());
            return Some(cycle);
        }
        for &v in &adj[u] {
            if members.contains(&v) && !parent.contains_key(&v) {
                parent.insert(v, u);
                queue.push_back(v);
            }
        }
    }
    None
}
