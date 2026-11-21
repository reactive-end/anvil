//! Parallel full-repo graph construction (`ignore` + `rayon`) and O(1) incremental updates.

mod collect;
mod walker;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use anvil_ast::analyze_file;
use anvil_core::config::AnvilConfig;
use anvil_core::traits::{EdgeRef, Language};
use rayon::prelude::*;
use rustc_hash::FxHashSet;

use crate::model::{CodeGraph, GraphEdge};
use crate::resolvers::{ResolutionContext, kotlin, resolve_file_edges};

pub use collect::collect_source_files;

/// Summary of changes applied during an incremental graph refresh.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IncrementalStats {
    pub added: usize,
    pub modified: usize,
    pub removed: usize,
    pub unchanged: usize,
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

    let mut pending: Vec<(Language, Vec<EdgeRef>)> = Vec::with_capacity(extracted.len());
    for item in extracted {
        let lang = item.facts.language;
        let edges = walker::insert_file_facts(&mut graph, item, false);
        pending.push((lang, edges));
    }
    graph.rebuild_indexes();

    let ctx = ResolutionContext::load(repo_root);
    let resolved_batches: Vec<Vec<GraphEdge>> = pending
        .par_iter()
        .map(|(lang, edges)| resolve_file_edges(*lang, edges, &graph, &ctx))
        .collect();

    for batch in resolved_batches {
        graph.edges.extend(batch);
    }
    graph.rebuild_indexes();
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
    if !abs.is_file() {
        return Ok(walker::remove_file_incremental(graph, &rel));
    }

    let bytes =
        fs::read(&abs).with_context(|| format!("failed to read {}", abs.display()))?;
    let hash = blake3::hash(&bytes).to_hex().to_string();
    if graph.file_hashes.get(&rel) == Some(&hash) {
        return Ok(false);
    }

    let source = String::from_utf8(bytes)
        .with_context(|| format!("non-UTF8 source file {}", abs.display()))?;
    let facts = analyze_file(Path::new(&rel), &source, config)?;
    let lang = facts.language;
    let kotlin_pkg = (lang == Language::Kotlin)
        .then(|| kotlin::extract_kotlin_package(&source))
        .flatten();

    walker::remove_file_incremental(graph, &rel);
    let raw_edges = walker::insert_file_facts(
        graph,
        walker::ExtractedFile { facts, kotlin_pkg },
        true,
    );

    let ctx = ResolutionContext::load(repo_root);
    let new_edges = resolve_file_edges(lang, &raw_edges, graph, &ctx);
    for edge in new_edges {
        walker::push_edge(graph, edge, true);
    }
    walker::reconcile_incoming_for_file(graph, &rel);

    if graph.tombstone_edges > graph.edges.len() / 4 {
        graph.compact_tombstones();
    }
    Ok(true)
}

/// Scans `repo_root`, hashes files in parallel, and re-parses only added or modified files.
pub fn update_incremental(
    graph: &mut CodeGraph,
    repo_root: &Path,
    config: &AnvilConfig,
) -> Result<IncrementalStats> {
    let files = collect_source_files(repo_root);
    let disk_set: FxHashSet<String> = files.iter().map(|(_, rel)| rel.clone()).collect();
    let removed_files: Vec<String> = graph
        .file_hashes
        .keys()
        .filter(|k| !disk_set.contains(*k))
        .cloned()
        .collect();

    let hashed: Vec<(PathBuf, String, bool)> = files
        .par_iter()
        .filter_map(|(abs, rel)| {
            let bytes = fs::read(abs).ok()?;
            let hash = blake3::hash(&bytes).to_hex().to_string();
            let is_new = !graph.file_hashes.contains_key(rel);
            let changed = graph.file_hashes.get(rel) != Some(&hash);
            changed.then(|| (abs.clone(), rel.clone(), is_new))
        })
        .collect();

    let mut stats = IncrementalStats {
        removed: removed_files.len(),
        unchanged: files.len().saturating_sub(hashed.len()),
        ..IncrementalStats::default()
    };

    if removed_files.is_empty() && hashed.is_empty() {
        return Ok(stats);
    }

    apply_incremental_changes(graph, repo_root, config, &removed_files, &hashed, &mut stats);
    Ok(stats)
}

fn apply_incremental_changes(
    graph: &mut CodeGraph,
    repo_root: &Path,
    config: &AnvilConfig,
    removed_files: &[String],
    hashed: &[(PathBuf, String, bool)],
    stats: &mut IncrementalStats,
) {
    for rem in removed_files {
        walker::remove_file_incremental(graph, rem);
    }

    let extracted: Vec<(walker::ExtractedFile, bool)> = hashed
        .par_iter()
        .filter_map(|(abs, rel, is_new)| {
            walker::extract_one(abs, rel, config)
                .ok()
                .flatten()
                .map(|ef| (ef, *is_new))
        })
        .collect();

    let mut pending = Vec::with_capacity(extracted.len());
    for (item, is_new) in extracted {
        if is_new {
            stats.added += 1;
        } else {
            stats.modified += 1;
        }
        let path = item.facts.path.clone();
        let lang = item.facts.language;
        walker::remove_file_incremental(graph, &path);
        let raw_edges = walker::insert_file_facts(graph, item, true);
        pending.push((lang, raw_edges));
    }

    let ctx = ResolutionContext::load(repo_root);
    let batches: Vec<Vec<GraphEdge>> = pending
        .par_iter()
        .map(|(lang, edges)| resolve_file_edges(*lang, edges, graph, &ctx))
        .collect();
    for batch in batches {
        graph.edges.extend(batch);
    }
    graph.compact_tombstones();
    graph.rebuild_indexes();
}
