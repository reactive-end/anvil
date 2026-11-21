//! Reverse-BFS blast radius analysis and test-impact mapping.

mod guard;
mod typecheck;

use std::collections::VecDeque;
use std::time::Instant;

use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;

use crate::model::{CodeGraph, GraphEdgeKind, NodeKind};

pub use guard::{OverengineeringReport, OverengineeringWarning, check_overengineering};
pub use typecheck::run_impacted_typecheck;

/// A consumer file impacted by a change, with BFS depth and causal edges.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ImpactedItem {
    pub file: String,
    pub depth: usize,
    pub via_edges: Vec<String>,
    pub is_screen: bool,
}

/// Result of reverse-BFS blast-radius computation.
#[derive(Debug, Clone, PartialEq, Serialize)]
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

    let mut visited_nodes: FxHashMap<String, usize> = FxHashMap::default();
    let mut queue: VecDeque<(String, usize)> = VecDeque::new();

    for file in &norm_changed {
        let fid = CodeGraph::file_node_id(file);
        visited_nodes.insert(fid.clone(), 0);
        queue.push_back((fid, 0));
        if let Some(node_ids) = graph.file_nodes.get(file) {
            for nid in node_ids {
                if !visited_nodes.contains_key(nid) {
                    visited_nodes.insert(nid.clone(), 0);
                    queue.push_back((nid.clone(), 0));
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
    collect_sibling_tests(graph, &norm_changed, &impacted_files, &mut related_tests);
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
    visited_nodes: &mut FxHashMap<String, usize>,
    queue: &mut VecDeque<(String, usize)>,
    discovered: &mut FxHashMap<String, (usize, Vec<String>)>,
) {
    while let Some((node_id, depth)) = queue.pop_front() {
        let Some(edge_idxs) = graph.reverse_adj.get(&node_id) else {
            continue;
        };
        for &idx in edge_idxs {
            let Some(edge) = graph.edges.get(idx) else {
                continue;
            };
            if edge.source.is_empty() || !edge.resolved {
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
            let Some(src_node) = graph.nodes.get(&edge.source) else {
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
                let reason = format!("{:?}: {}", edge.kind, edge.raw_target);
                if !entry.1.contains(&reason) {
                    entry.1.push(reason);
                }
            }

            let prev_best = visited_nodes.get(&edge.source).copied().unwrap_or(usize::MAX);
            if next_depth < prev_best {
                visited_nodes.insert(edge.source.clone(), next_depth);
                queue.push_back((edge.source.clone(), next_depth));
                if src_node.kind == NodeKind::Symbol && !src_node.file.is_empty() {
                    let owner_file_id = CodeGraph::file_node_id(&src_node.file);
                    let owner_best = visited_nodes.get(&owner_file_id).copied().unwrap_or(usize::MAX);
                    if next_depth < owner_best {
                        visited_nodes.insert(owner_file_id.clone(), next_depth);
                        queue.push_back((owner_file_id, next_depth));
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
        if is_test_file(&file) {
            tests.push(file);
        } else {
            let is_screen = is_screen_file(&file);
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

fn collect_sibling_tests(
    graph: &CodeGraph,
    changed: &[String],
    impacted: &[ImpactedItem],
    tests: &mut Vec<String>,
) {
    let mut stems = Vec::new();
    for f in changed.iter().chain(impacted.iter().map(|i| &i.file)) {
        if !is_test_file(f)
            && let Some(stem) = file_stem_name(f)
        {
            stems.push(stem);
        }
    }
    for candidate in graph.file_hashes.keys() {
        if is_test_file(candidate)
            && let Some(c_stem) = test_base_stem(candidate)
            && stems.iter().any(|s| s == &c_stem)
        {
            tests.push(candidate.clone());
        }
    }
}

fn is_test_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let fname = lower.rsplit('/').next().unwrap_or(&lower);
    lower.contains(".test.")
        || lower.contains(".spec.")
        || lower.contains("/__tests__/")
        || fname.starts_with("test_")
        || fname.ends_with("_test.py")
        || fname.ends_with("_test.dart")
        || fname.ends_with("test.kt")
}

fn is_screen_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("screen")
        || lower.contains("page")
        || lower.ends_with(".component.html")
        || lower.contains("/views/")
}

fn file_stem_name(path: &str) -> Option<String> {
    let fname = path.rsplit('/').next()?;
    Some(fname.split('.').next()?.to_ascii_lowercase())
}

fn test_base_stem(path: &str) -> Option<String> {
    let fname = path.rsplit('/').next()?.to_ascii_lowercase();
    for suffix in [".test.", ".spec."] {
        if let Some((stem, _)) = fname.split_once(suffix) {
            return Some(stem.to_owned());
        }
    }
    if let Some(rest) = fname.strip_prefix("test_") {
        return rest.split('.').next().map(str::to_owned);
    }
    for suffix in ["_test.py", "_test.dart", "test.kt"] {
        if let Some(stem) = fname.strip_suffix(suffix) {
            return Some(stem.to_owned());
        }
    }
    None
}
