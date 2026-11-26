//! `SymbolFreshnessProvider` implementations for `CodeGraph` and `CodeGraphEngine`.

use std::fs;

use anvil_core::traits::{FreshnessReport, FreshnessState, SymbolFreshnessProvider};
use anyhow::Result;

use crate::CodeGraphEngine;
use crate::model::CodeGraph;

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
                    reason: Some(format!(
                        "Referenced file `{norm}` no longer exists in code graph"
                    )),
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
        Ok(FreshnessReport { state: FreshnessState::Fresh, reason: None })
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
