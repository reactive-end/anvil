//! Reverse-BFS blast radius analysis and test-impact mapping.

mod boundaries;
mod guard;
mod sibling;
mod tests_native;
mod tests_runner;
mod typecheck;
mod typecheck_filter;
mod typecheck_godot;
mod typecheck_php;
mod typecheck_ts;

use std::collections::VecDeque;
use std::time::Instant;

use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};

use crate::model::{CodeGraph, GraphEdgeKind, NodeId, NodeKind};

pub use boundaries::check_boundaries;
pub use guard::{OverengineeringReport, OverengineeringWarning, check_overengineering};
pub use tests_runner::run_impacted_tests;
pub use typecheck::{
    build_ts_typecheck_command, build_vue_or_ts_typecheck_command, filter_typecheck_output,
    run_impacted_typecheck,
};

/// A consumer file impacted by a change, with BFS depth and causal edges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImpactedItem {
    pub file: String,
    pub depth: usize,
    pub via_edges: Vec<String>,
    pub is_screen: bool,
}

/// Result of reverse-BFS blast-radius computation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlastRadiusReport {
    pub changed_files: Vec<String>,
    pub max_depth: usize,
    pub impacted_files: Vec<ImpactedItem>,
    pub related_tests: Vec<String>,
    pub elapsed_ms: f64,
}

/// Computes the exact reverse-BFS blast radius and related test files up to `max_depth`.
pub fn compute_blast_radius(
    graph: &CodeGraph,
    changed_files: &[String],
    max_depth: usize,
) -> BlastRadiusReport {
    let started = Instant::now();
    let norm_changed: Vec<String> = changed_files
        .iter()
        .map(|f| CodeGraph::normalize_rel_path(f))
        .collect();
    let changed_set: FxHashSet<&str> = norm_changed.iter().map(String::as_str).collect();

    let mut visited_nodes: FxHashMap<NodeId, usize> = FxHashMap::default();
    let mut queue: VecDeque<(NodeId, usize)> = VecDeque::new();

    for file in &norm_changed {
        let fid = CodeGraph::file_node_id(file);
        if let Some((fnid, _)) = graph.node_by_id(&fid)
            && visited_nodes.insert(fnid, 0).is_none()
        {
            queue.push_back((fnid, 0));
        }
        if let Some(node_ids) = graph.file_nodes.get(file) {
            for &nid in node_ids {
                if visited_nodes.insert(nid, 0).is_none() {
                    queue.push_back((nid, 0));
                }
            }
        }
    }

    let mut discovered: FxHashMap<String, (usize, Vec<String>)> = FxHashMap::default();
    run_reverse_bfs(
        graph,
        max_depth,
        &changed_set,
        &mut visited_nodes,
        &mut queue,
        &mut discovered,
    );

    let (impacted_files, mut related_tests) = split_consumers_and_tests(discovered);
    sibling::collect_sibling_tests(graph, &norm_changed, &impacted_files, &mut related_tests);
    related_tests.sort();
    related_tests.dedup();

    BlastRadiusReport {
        changed_files: norm_changed,
        max_depth,
        impacted_files,
        related_tests,
        elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
    }
}

fn run_reverse_bfs(
    graph: &CodeGraph,
    max_depth: usize,
    changed_set: &FxHashSet<&str>,
    visited_nodes: &mut FxHashMap<NodeId, usize>,
    queue: &mut VecDeque<(NodeId, usize)>,
    discovered: &mut FxHashMap<String, (usize, Vec<String>)>,
) {
    while let Some((nid, depth)) = queue.pop_front() {
        let Some(edge_idxs) = graph.reverse_adj.get(nid as usize) else {
            continue;
        };
        for &idx in edge_idxs {
            let Some(edge) = graph.edges.get(idx as usize) else {
                continue;
            };
            if !edge.is_resolved() {
                continue;
            }
            let next_depth = if edge.kind == GraphEdgeKind::Contains {
                depth
            } else {
                depth + 1
            };
            if next_depth > max_depth {
                continue;
            }
            let Some(src_node) = graph.get_node(edge.source) else {
                continue;
            };
            if edge.kind != GraphEdgeKind::Contains
                && !src_node.file.is_empty()
                && !changed_set.contains(src_node.file.as_str())
            {
                let entry = discovered
                    .entry(src_node.file.clone())
                    .or_insert_with(|| (next_depth, Vec::new()));
                entry.0 = entry.0.min(next_depth);
                let target_lbl = graph.edge_target_label(edge);
                let reason = format!("{:?}: {}", edge.kind, target_lbl);
                if !entry.1.contains(&reason) {
                    entry.1.push(reason);
                }
            }

            let prev_best = visited_nodes.get(&edge.source).copied().unwrap_or(usize::MAX);
            if next_depth < prev_best {
                visited_nodes.insert(edge.source, next_depth);
                queue.push_back((edge.source, next_depth));
                if src_node.kind == NodeKind::Symbol && !src_node.file.is_empty() {
                    let owner_fid = CodeGraph::file_node_id(&src_node.file);
                    if let Some((owner_nid, _)) = graph.node_by_id(&owner_fid) {
                        let owner_best =
                            visited_nodes.get(&owner_nid).copied().unwrap_or(usize::MAX);
                        if next_depth < owner_best {
                            visited_nodes.insert(owner_nid, next_depth);
                            queue.push_back((owner_nid, next_depth));
                        }
                    }
                }
            }
        }
    }
}

fn split_consumers_and_tests(
    discovered: FxHashMap<String, (usize, Vec<String>)>,
) -> (Vec<ImpactedItem>, Vec<String>) {
    let mut impacted = Vec::new();
    let mut tests = Vec::new();
    for (file, (depth, via_edges)) in discovered {
        if sibling::is_test_file(&file) {
            tests.push(file);
        } else {
            let is_screen = sibling::is_screen_file(&file);
            impacted.push(ImpactedItem {
                file,
                depth,
                via_edges,
                is_screen,
            });
        }
    }
    impacted.sort_by(|a, b| a.depth.cmp(&b.depth).then_with(|| a.file.cmp(&b.file)));
    (impacted, tests)
}
