//! Architecture layer boundary checks and import cycle detection.

use anvil_core::config::AnvilConfig;
use anvil_core::traits::{Diagnostic, Severity};
use rustc_hash::{FxHashMap, FxHashSet};

use crate::model::{CodeGraph, GraphEdgeKind, NodeKind};

const MAX_CYCLE_DEPTH: usize = 64;
const MAX_REPORTED_CYCLES: usize = 50;

/// Checks configured boundary rules and optional import cycles on the resolved graph.
pub fn check_boundaries(
    graph: &CodeGraph,
    config: &AnvilConfig,
    scope_files: &[String],
) -> Vec<Diagnostic> {
    if config.boundaries.is_empty() && !config.forbid_import_cycles {
        return Vec::new();
    }

    let adj = build_import_adj(graph);
    let scoped = normalize_scope(scope_files, &adj);
    let mut diagnostics = Vec::new();

    if !config.boundaries.is_empty() {
        collect_boundary_violations(&adj, config, &scoped, &mut diagnostics);
    }
    if config.forbid_import_cycles {
        collect_import_cycles(&adj, &scoped, &mut diagnostics);
    }
    diagnostics
}

fn build_import_adj(graph: &CodeGraph) -> FxHashMap<String, Vec<String>> {
    let mut adj: FxHashMap<String, Vec<String>> = FxHashMap::default();
    for edge in &graph.edges {
        if !edge.is_resolved() || edge.kind != GraphEdgeKind::Imports {
            continue;
        }
        let Some(src_node) = graph.get_node(edge.source) else {
            continue;
        };
        let Some(dst_node) = graph.get_node(edge.target) else {
            continue;
        };
        if src_node.kind == NodeKind::Semantic || dst_node.kind == NodeKind::Semantic {
            continue;
        }
        if src_node.file.is_empty() || dst_node.file.is_empty() || src_node.file == dst_node.file {
            continue;
        }
        let targets = adj.entry(src_node.file.clone()).or_default();
        if !targets.contains(&dst_node.file) {
            targets.push(dst_node.file.clone());
        }
    }
    for targets in adj.values_mut() {
        targets.sort();
    }
    adj
}

fn normalize_scope(scope_files: &[String], adj: &FxHashMap<String, Vec<String>>) -> Vec<String> {
    let mut files: Vec<String> = if scope_files.is_empty() {
        adj.keys().cloned().collect()
    } else {
        scope_files
            .iter()
            .map(|f| CodeGraph::normalize_rel_path(f))
            .collect()
    };
    files.sort();
    files.dedup();
    files
}

fn collect_boundary_violations(
    adj: &FxHashMap<String, Vec<String>>,
    config: &AnvilConfig,
    scoped: &[String],
    diagnostics: &mut Vec<Diagnostic>,
) {
    for src in scoped {
        let Some(targets) = adj.get(src) else {
            continue;
        };
        for dst in targets {
            for rule in &config.boundaries {
                if glob_match(&rule.from_glob, src) && glob_match(&rule.forbid_glob, dst) {
                    let suggestion = rule.reason.clone().or_else(|| {
                        Some(format!(
                            "Remove dependency on `{dst}` to respect boundary `{}`",
                            rule.name
                        ))
                    });
                    diagnostics.push(Diagnostic {
                        rule: format!("boundary:{}", rule.name),
                        severity: Severity::Error,
                        file: src.clone(),
                        line: 1,
                        column: 1,
                        message: format!(
                            "forbidden import from `{src}` to `{dst}` (boundary `{}`)",
                            rule.name
                        ),
                        suggestion,
                    });
                }
            }
        }
    }
}

fn collect_import_cycles(
    adj: &FxHashMap<String, Vec<String>>,
    scoped: &[String],
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut color: FxHashMap<&str, u8> = FxHashMap::default();
    let mut seen_cycles: FxHashSet<String> = FxHashSet::default();
    for start in scoped {
        if seen_cycles.len() >= MAX_REPORTED_CYCLES {
            break;
        }
        if color.get(start.as_str()).copied().unwrap_or(0) != 0 {
            continue;
        }
        dfs_cycles_from(start.as_str(), adj, &mut color, &mut seen_cycles, diagnostics);
    }
}

struct DfsFrame<'a> {
    node: &'a str,
    next_idx: usize,
}

fn dfs_cycles_from<'a>(
    start: &'a str,
    adj: &'a FxHashMap<String, Vec<String>>,
    color: &mut FxHashMap<&'a str, u8>,
    seen_cycles: &mut FxHashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut path = vec![start];
    let mut stack = vec![DfsFrame { node: start, next_idx: 0 }];
    color.insert(start, 1);

    while let Some(frame) = stack.last_mut() {
        if seen_cycles.len() >= MAX_REPORTED_CYCLES {
            break;
        }
        let u = frame.node;
        let neighbors = adj.get(u).map(Vec::as_slice).unwrap_or(&[]);
        if frame.next_idx >= neighbors.len() || path.len() >= MAX_CYCLE_DEPTH {
            stack.pop();
            path.pop();
            color.insert(u, 2);
            continue;
        }
        let v = neighbors[frame.next_idx].as_str();
        frame.next_idx += 1;
        match color.get(v).copied().unwrap_or(0) {
            0 => {
                color.insert(v, 1);
                path.push(v);
                stack.push(DfsFrame { node: v, next_idx: 0 });
            }
            1 => record_cycle(&path, v, seen_cycles, diagnostics),
            _ => {}
        }
    }
}

fn record_cycle(
    path: &[&str],
    target: &str,
    seen_cycles: &mut FxHashSet<String>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let Some(pos) = path.iter().position(|&n| n == target) else {
        return;
    };
    let slice = &path[pos..];
    if slice.is_empty() {
        return;
    }
    let min_idx = slice
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.cmp(b.1))
        .map_or(0, |(idx, _)| idx);
    let mut canonical: Vec<&str> = Vec::with_capacity(slice.len() + 1);
    canonical.extend_from_slice(&slice[min_idx..]);
    canonical.extend_from_slice(&slice[..min_idx]);
    canonical.push(slice[min_idx]);

    let cycle_str = canonical.join(" -> ");
    if seen_cycles.insert(cycle_str.clone()) {
        diagnostics.push(Diagnostic {
            rule: "architecture:import-cycle".to_owned(),
            severity: Severity::Error,
            file: canonical[0].to_owned(),
            line: 1,
            column: 1,
            message: format!("import cycle detected: {cycle_str}"),
            suggestion: Some(
                "Break the circular import using dependency inversion or a shared module"
                    .to_owned(),
            ),
        });
    }
}

fn glob_match(pattern: &str, path: &str) -> bool {
    let p_norm = CodeGraph::normalize_rel_path(pattern);
    let s_norm = CodeGraph::normalize_rel_path(path);
    let p_segs: Vec<&str> = p_norm.split('/').filter(|s| !s.is_empty()).collect();
    let s_segs: Vec<&str> = s_norm.split('/').filter(|s| !s.is_empty()).collect();
    let (mut pi, mut si, mut star_pi, mut star_si) = (0usize, 0usize, None, 0usize);
    while si < s_segs.len() {
        if pi < p_segs.len() && p_segs[pi] == "**" {
            star_pi = Some(pi);
            star_si = si;
            pi += 1;
        } else if pi < p_segs.len() && seg_match(p_segs[pi], s_segs[si]) {
            pi += 1;
            si += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_si += 1;
            si = star_si;
        } else {
            return false;
        }
    }
    while pi < p_segs.len() && p_segs[pi] == "**" {
        pi += 1;
    }
    pi == p_segs.len()
}

fn seg_match(pat: &str, text: &str) -> bool {
    let (p, t) = (pat.as_bytes(), text.as_bytes());
    let (mut pi, mut ti, mut star_pi, mut star_ti) = (0usize, 0usize, None, 0usize);
    while ti < t.len() {
        if pi < p.len() && p[pi] == b'*' {
            star_pi = Some(pi);
            star_ti = ti;
            pi += 1;
        } else if pi < p.len() && (p[pi] == b'?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if let Some(sp) = star_pi {
            pi = sp + 1;
            star_ti += 1;
            ti = star_ti;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}
