//! Incremental code graph engine, framework-aware resolvers, analytics, and blast-radius analyzer.

pub mod analytics;
pub mod builder;
pub mod graphify_compat;
pub mod impact;
pub mod merge;
pub mod model;
pub mod query;
pub mod resolvers;
pub mod store;

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use anvil_core::config::AnvilConfig;
use anvil_core::traits::{CompressedOutput, FreshnessReport, FreshnessState, SymbolFreshnessProvider};

use crate::analytics::{CommunitySummary, GodNode};
use crate::builder::IncrementalStats;
use crate::graphify_compat::OverlayStats;
use crate::impact::{BlastRadiusReport, OverengineeringReport};
use crate::model::CodeGraph;
use crate::query::{ExplainResult, OriginResult, PathResult, QueryHit};

/// Crate version marker for workspace smoke checks.
pub fn crate_name() -> &'static str {
    "anvil-graph"
}

/// High-level facade managing persistence, incremental updates, queries, and blast-radius analysis.
pub struct CodeGraphEngine {
    pub repo_root: PathBuf,
    pub config: AnvilConfig,
    pub graph: CodeGraph,
}

impl CodeGraphEngine {
    /// Builds a fresh code graph from `repo_root` without reading prior cached state.
    pub fn build(repo_root: impl AsRef<Path>, config: AnvilConfig) -> Result<Self> {
        let root = repo_root.as_ref().to_path_buf();
        let graph = builder::build_full(&root, &config)?;
        Ok(Self {
            repo_root: root,
            config,
            graph,
        })
    }

    /// Pure binary load from `.harness/.state/graph.bin` (zero walk, zero hashing, zero communities).
    /// Falls back to `build_full` only if `graph.bin` is absent or corrupt.
    pub fn load_cached(repo_root: impl AsRef<Path>, config: AnvilConfig) -> Result<Self> {
        let root = repo_root.as_ref().to_path_buf();
        let bin_path = store::default_bin_path(&root);
        let graph = if bin_path.is_file() {
            match store::load_binary(&bin_path) {
                Ok(g) => g,
                Err(err) => {
                    eprintln!("warning: corrupt graph cache {}: {err}; rebuilding", bin_path.display());
                    builder::build_full(&root, &config)?
                }
            }
        } else {
            builder::build_full(&root, &config)?
        };
        Ok(Self { repo_root: root, config, graph })
    }

    /// Alias for `load_cached` for pure binary loads prior to explicit incremental updates.
    pub fn load_binary_only(repo_root: impl AsRef<Path>, config: AnvilConfig) -> Result<Self> {
        Self::load_cached(repo_root, config)
    }

    /// Loads `.harness/.state/graph.bin` and refreshes changed files via `(mtime, len)` fast-path.
    pub fn load_or_build(repo_root: impl AsRef<Path>, config: AnvilConfig) -> Result<Self> {
        let root = repo_root.as_ref().to_path_buf();
        let bin_path = store::default_bin_path(&root);
        let graph = if bin_path.is_file() {
            match store::load_binary(&bin_path) {
                Ok(mut g) => {
                    let stats = builder::update_incremental(&mut g, &root, &config)?;
                    if stats.added > 0 || stats.modified > 0 || stats.removed > 0 {
                        store::save_binary(&g, &bin_path)?;
                    }
                    g
                }
                Err(err) => {
                    eprintln!("warning: corrupt graph cache {}: {err}; rebuilding", bin_path.display());
                    builder::build_full(&root, &config)?
                }
            }
        } else {
            builder::build_full(&root, &config)?
        };
        Ok(Self { repo_root: root, config, graph })
    }

    /// Saves the binary graph atomically to `.harness/.state/graph.bin`.
    pub fn save(&self) -> Result<PathBuf> {
        let path = store::default_bin_path(&self.repo_root);
        store::save_binary(&self.graph, &path)?;
        Ok(path)
    }

    /// Exports the graph in NetworkX node-link JSON format to `.harness/.state/graph.json`.
    pub fn export_json(&self) -> Result<PathBuf> {
        let path = store::default_json_path(&self.repo_root);
        store::export_networkx_json(&self.graph, &path)?;
        Ok(path)
    }

    /// Exports a lightweight static HTML visualization to `.harness/.state/graph.html`.
    pub fn export_html(&self, path: &Path) -> Result<()> {
        analytics::export_html(&self.graph, path)
    }

    /// Incrementally updates a single modified/added/deleted file in the live graph.
    pub fn update_file(&mut self, rel_path: &str) -> Result<bool> {
        builder::update_file(&mut self.graph, &self.repo_root, rel_path, &self.config)
    }

    /// Incrementally refreshes modified files using `(mtime, len)` + `blake3` fast-path.
    pub fn update_incremental(&mut self) -> Result<IncrementalStats> {
        builder::update_incremental(&mut self.graph, &self.repo_root, &self.config)
    }

    /// Runs BM25/token-scored symbol and file search.
    pub fn query(&self, term: &str, limit: usize) -> Vec<QueryHit> {
        query::query(&self.graph, term, limit)
    }

    /// Finds the shortest directed dependency path between two symbols or files.
    pub fn shortest_path(&self, from: &str, to: &str) -> Option<PathResult> {
        query::shortest_path(&self.graph, from, to)
    }

    /// Explains incoming/outgoing neighbors, community, and god-node status for `target`.
    pub fn explain(&self, target: &str) -> Option<ExplainResult> {
        query::explain(&self.graph, target)
    }

    /// Traces upstream definition/source-of-truth for anti-slop edits.
    pub fn find_origin(&self, target: &str) -> Option<OriginResult> {
        query::find_origin(&self.graph, target)
    }

    /// Computes reverse-BFS blast radius and mapped test files from `changed_files`.
    pub fn blast_radius(&self, changed_files: &[String], max_depth: usize) -> BlastRadiusReport {
        impact::compute_blast_radius(&self.graph, changed_files, max_depth)
    }

    /// Checks whether a small fix introduced new abstractions with <= 1 consumer.
    pub fn check_overengineering(
        &self,
        changed_files: &[String],
        added_files: &[String],
        is_small_fix: bool,
    ) -> OverengineeringReport {
        impact::check_overengineering(&self.graph, changed_files, added_files, is_small_fix)
    }

    /// Invokes native typecheckers filtered to `impacted_files` via `anvil-core` compressor.
    pub fn run_impacted_typecheck(&self, impacted_files: &[String]) -> Result<Vec<CompressedOutput>> {
        impact::run_impacted_typecheck(&self.repo_root, &self.config, impacted_files)
    }

    /// Runs targeted test runners for `related_tests` via `anvil-core` compressor.
    pub fn run_impacted_tests(&self, related_tests: &[String]) -> Result<Vec<CompressedOutput>> {
        impact::run_impacted_tests(&self.repo_root, &self.config, related_tests)
    }

    /// Computes communities and generates a compact `GRAPH_REPORT.md` string.
    pub fn report_markdown(&mut self) -> String {
        analytics::generate_report(&mut self.graph)
    }

    /// Runs deterministic community detection on the graph.
    pub fn detect_communities(&mut self) -> Vec<CommunitySummary> {
        analytics::detect_communities(&mut self.graph)
    }

    /// Returns top `top_n` God Nodes ranked by dependency degree centrality.
    pub fn god_nodes(&self, top_n: usize) -> Vec<GodNode> {
        analytics::find_god_nodes(&self.graph, top_n)
    }

    /// Returns file-level import/re-export cycles.
    pub fn cycles(&self, max_cycles: usize) -> Vec<Vec<String>> {
        analytics::detect_cycles(&self.graph, max_cycles)
    }

    /// Overlays semantic/rationale nodes from an existing `graphify-out/graph.json`.
    pub fn apply_graphify_overlay(&mut self, json_path: &Path) -> Result<OverlayStats> {
        graphify_compat::load_graphify_overlay(&mut self.graph, json_path)
    }
}

impl SymbolFreshnessProvider for CodeGraph {
    fn check_freshness(
        &self,
        related_files: &[String],
        related_symbols: &[String],
    ) -> Result<FreshnessReport> {
        for raw_file in related_files {
            let norm = Self::normalize_rel_path(raw_file);
            if !self.file_hashes.contains_key(&norm) {
                return Ok(FreshnessReport {
                    state: FreshnessState::Stale,
                    reason: Some(format!("Referenced file `{norm}` no longer exists in code graph")),
                });
            }
        }
        for sym in related_symbols {
            if !has_symbol(self, sym) {
                return Ok(FreshnessReport {
                    state: FreshnessState::Stale,
                    reason: Some(format!("Referenced symbol `{sym}` was not found in code graph")),
                });
            }
        }
        Ok(FreshnessReport {
            state: FreshnessState::Fresh,
            reason: None,
        })
    }
}

impl SymbolFreshnessProvider for CodeGraphEngine {
    fn check_freshness(
        &self,
        related_files: &[String],
        related_symbols: &[String],
    ) -> Result<FreshnessReport> {
        let base = self.graph.check_freshness(related_files, related_symbols)?;
        if base.state != FreshnessState::Fresh {
            return Ok(base);
        }
        for raw_file in related_files {
            let norm = CodeGraph::normalize_rel_path(raw_file);
            let disk_path = self.repo_root.join(&norm);
            let Ok(bytes) = fs::read(&disk_path) else {
                return Ok(FreshnessReport {
                    state: FreshnessState::Stale,
                    reason: Some(format!("File `{norm}` is missing on disk")),
                });
            };
            let live_hash = blake3::hash(&bytes).to_hex().to_string();
            if self.graph.file_hashes.get(&norm) != Some(&live_hash) {
                return Ok(FreshnessReport {
                    state: FreshnessState::NeedsVerification,
                    reason: Some(format!("File `{norm}` changed on disk since last graph update")),
                });
            }
        }
        Ok(base)
    }
}

fn has_symbol(graph: &CodeGraph, sym: &str) -> bool {
    if graph.contains_node_id(sym) || graph.symbol_index.contains_key(sym) {
        return true;
    }
    let suffix = format!("::{sym}");
    graph.nodes.iter().any(|n| {
        !n.is_tombstone() && (n.label.eq_ignore_ascii_case(sym) || n.id.ends_with(&suffix))
    })
}
