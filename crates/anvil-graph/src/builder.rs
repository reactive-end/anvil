//! Parallel full-repo graph construction (`ignore` + `rayon`) and O(1) incremental updates.

mod collect;
mod remove;
mod walker;

use std::fs;
use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_core::traits::{EdgeRef, Language};
use anyhow::{Context, Result};
use rayon::prelude::*;
use rustc_hash::FxHashSet;

use crate::model::CodeGraph;
use crate::resolvers::{ResolutionContext, ResolvedEdge, http_routes, resolve_file_edges};

pub use collect::collect_source_files;

/// Summary of changes applied during an incremental graph refresh.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IncrementalStats {
    pub added: usize,
    pub modified: usize,
    pub removed: usize,
    pub unchanged: usize,
}

enum IncrementalScanResult {
    StatOnly { rel: String, stat: (u64, u64) },
    Extracted { item: walker::ExtractedFile, is_new: bool },
}

/// Builds a complete `CodeGraph` for `repo_root` using `.gitignore`-aware parallel extraction.
pub fn build_full(repo_root: &Path, config: &AnvilConfig) -> Result<CodeGraph> {
    let files = collect_source_files(repo_root);
    let extracted: Vec<walker::ExtractedFile> = files
        .par_iter()
        .filter_map(|(abs, rel)| walker::extract_one(abs, rel, config).ok().flatten())
        .collect();

    let norm_root = CodeGraph::normalize_rel_path(&repo_root.to_string_lossy());
    let mut graph = CodeGraph::new(norm_root);
    graph.nodes.reserve(extracted.len() * 4);
    graph.file_hashes.reserve(extracted.len());
    graph.file_stats.reserve(extracted.len());

    let mut pending: Vec<(Language, Vec<EdgeRef>)> = Vec::with_capacity(extracted.len());
    for item in extracted {
        let lang = item.facts.language;
        let edges = walker::insert_file_facts(&mut graph, item, false, None);
        pending.push((lang, edges));
    }
    graph.rebuild_indexes();
    http_routes::materialize_exposed_routes(&mut graph, &pending, false);

    let ctx = ResolutionContext::load(repo_root);
    let resolved_batches: Vec<Vec<ResolvedEdge>> = pending
        .par_iter()
        .map(|(lang, edges)| resolve_file_edges(*lang, edges, &graph, &ctx))
        .collect();

    for batch in resolved_batches {
        walker::push_resolved_edges(&mut graph, batch, false);
    }
    graph.rebuild_adjacency();
    let _ = crate::analytics::detect_communities(&mut graph);
    Ok(graph)
}

/// Incrementally updates ONLY `rel_path` in `graph` in O(file_degree); returns `true` if changed.
pub fn update_file(
    graph: &mut CodeGraph,
    repo_root: &Path,
    rel_path: &str,
    config: &AnvilConfig,
) -> Result<bool> {
    let rel = CodeGraph::normalize_rel_path(rel_path);
    let abs = repo_root.join(&rel);
    if !abs.is_file() || !collect::is_supported_source(&abs) {
        let (removed, unused) = walker::remove_file_incremental(graph, &rel);
        walker::finalize_unused_slots(graph, unused);
        if removed {
            maybe_compact(graph);
        }
        return Ok(removed);
    }

    let meta = fs::metadata(&abs).with_context(|| format!("failed to stat {}", abs.display()))?;
    if meta.len() > walker::MAX_FILE_BYTES {
        return Ok(false);
    }
    let stat = walker::file_stat_from_meta(&meta);
    let bytes = fs::read(&abs).with_context(|| format!("failed to read {}", abs.display()))?;
    if bytes.contains(&0) {
        return Ok(false);
    }
    let hash = blake3::hash(&bytes).to_hex().to_string();
    if graph.file_hashes.get(&rel) == Some(&hash) {
        graph.file_stats.insert(rel, stat);
        return Ok(false);
    }

    let Some(item) = walker::extract_from_bytes(&rel, bytes, stat, config)? else {
        return Ok(false);
    };
    let lang = item.facts.language;

    let (_, mut reuse_slots) = walker::remove_file_incremental(graph, &rel);
    let raw_edges = walker::insert_file_facts(graph, item, true, Some(&mut reuse_slots));
    walker::finalize_unused_slots(graph, reuse_slots);
    http_routes::materialize_exposed_routes_for_file(graph, &raw_edges, true);

    let ctx = ResolutionContext::load(repo_root);
    let new_edges = resolve_file_edges(lang, &raw_edges, graph, &ctx);
    walker::push_resolved_edges(graph, new_edges, true);
    http_routes::link_external_route_calls(graph, true);

    maybe_compact(graph);
    Ok(true)
}

fn maybe_compact(graph: &mut CodeGraph) {
    if graph.tombstone_edges > graph.edges.len() / 4
        || graph.tombstone_nodes > graph.nodes.len() / 4
    {
        graph.compact_tombstones();
    }
}

/// Scans `repo_root` with `(mtime, len)` fast-path and re-parses only added or modified files.
pub fn update_incremental(
    graph: &mut CodeGraph,
    repo_root: &Path,
    config: &AnvilConfig,
) -> Result<IncrementalStats> {
    let files = collect_source_files(repo_root);
    let disk_set: FxHashSet<String> = files.iter().map(|(_, rel)| rel.clone()).collect();
    let removed_files: Vec<String> =
        graph.file_hashes.keys().filter(|k| !disk_set.contains(*k)).cloned().collect();

    let scanned: Vec<IncrementalScanResult> = files
        .par_iter()
        .filter_map(|(abs, rel)| scan_one_incremental(abs, rel, graph, config))
        .collect();

    let mut stat_updates = Vec::new();
    let mut extracted = Vec::new();
    for entry in scanned {
        match entry {
            IncrementalScanResult::StatOnly { rel, stat } => stat_updates.push((rel, stat)),
            IncrementalScanResult::Extracted { item, is_new } => extracted.push((item, is_new)),
        }
    }

    for (rel, stat) in stat_updates {
        graph.file_stats.insert(rel, stat);
    }

    let mut stats = IncrementalStats {
        removed: removed_files.len(),
        unchanged: files.len().saturating_sub(extracted.len()),
        ..IncrementalStats::default()
    };

    if removed_files.is_empty() && extracted.is_empty() {
        return Ok(stats);
    }

    apply_incremental_changes(graph, repo_root, &removed_files, extracted, &mut stats);
    Ok(stats)
}

fn scan_one_incremental(
    abs: &Path,
    rel: &str,
    graph: &CodeGraph,
    config: &AnvilConfig,
) -> Option<IncrementalScanResult> {
    let meta = fs::metadata(abs).ok()?;
    if meta.len() > walker::MAX_FILE_BYTES {
        return None;
    }
    let stat = walker::file_stat_from_meta(&meta);
    let is_new = !graph.file_hashes.contains_key(rel);
    if !is_new && stat.0 != 0 && graph.file_stats.get(rel) == Some(&stat) {
        return None;
    }

    let bytes = fs::read(abs).ok()?;
    let hash = blake3::hash(&bytes).to_hex().to_string();
    if graph.file_hashes.get(rel) == Some(&hash) {
        return Some(IncrementalScanResult::StatOnly { rel: rel.to_owned(), stat });
    }

    let item = walker::extract_from_bytes(rel, bytes, stat, config).ok().flatten()?;
    Some(IncrementalScanResult::Extracted { item, is_new })
}

fn apply_incremental_changes(
    graph: &mut CodeGraph,
    repo_root: &Path,
    removed_files: &[String],
    extracted: Vec<(walker::ExtractedFile, bool)>,
    stats: &mut IncrementalStats,
) {
    for rem in removed_files {
        let (_, unused) = walker::remove_file_incremental(graph, rem);
        walker::finalize_unused_slots(graph, unused);
    }

    let mut pending = Vec::with_capacity(extracted.len());
    for (item, is_new) in extracted {
        if is_new {
            stats.added += 1;
        } else {
            stats.modified += 1;
        }
        let path = item.facts.path.clone();
        let lang = item.facts.language;
        let (_, mut reuse_slots) = walker::remove_file_incremental(graph, &path);
        let raw_edges = walker::insert_file_facts(graph, item, true, Some(&mut reuse_slots));
        walker::finalize_unused_slots(graph, reuse_slots);
        pending.push((lang, raw_edges));
    }

    http_routes::materialize_exposed_routes(graph, &pending, false);
    let ctx = ResolutionContext::load(repo_root);
    let batches: Vec<Vec<ResolvedEdge>> = pending
        .par_iter()
        .map(|(lang, edges)| resolve_file_edges(*lang, edges, graph, &ctx))
        .collect();
    for batch in batches {
        walker::push_resolved_edges(graph, batch, false);
    }
    http_routes::link_external_route_calls(graph, false);
    if !graph.compact_tombstones() {
        graph.rebuild_adjacency();
    }
}
