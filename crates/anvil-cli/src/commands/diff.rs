//! `anvil diff`: noise-filtered, token-bounded `git diff` with affected symbol annotations.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use anvil_core::diff_grep::{compress_diff, is_noise_file};
use anvil_core::stats::{StatsEvent, record_event};
use anvil_graph::model::{CodeGraph, NodeKind};
use anvil_graph::store;

/// Executes `anvil diff [--max-lines N] [--max-tokens N]`.
pub fn execute(root: &Path, max_lines: usize, max_tokens: usize) -> Result<()> {
    let raw_diff = run_git_diff_head(root)?;
    let compressed = compress_diff(&raw_diff, max_lines, max_tokens);

    if compressed.raw_bytes > compressed.compact_bytes {
        let _ = record_event(
            root,
            StatsEvent::CompressorSaved {
                raw_bytes: compressed.raw_bytes,
                compact_bytes: compressed.compact_bytes,
            },
        );
    }

    if compressed.text.is_empty() && compressed.skipped_files.is_empty() {
        println!("no diff changes");
        return Ok(());
    }

    let touched = collect_touched_files(&raw_diff);
    print_affected_symbols(root, &touched);
    if !compressed.text.is_empty() {
        println!("{}", compressed.text);
    }
    println!(
        "# diff: raw_bytes={} compact_bytes={} tokens_saved={} skipped={} truncated={}",
        compressed.raw_bytes,
        compressed.compact_bytes,
        compressed.estimated_tokens_saved,
        compressed.skipped_files.len(),
        compressed.truncated
    );
    Ok(())
}

fn run_git_diff_head(root: &Path) -> Result<String> {
    let head_attempt = Command::new("git")
        .args(["-c", "core.quotePath=false", "diff", "HEAD"])
        .current_dir(root)
        .output()
        .context("failed to execute git diff HEAD")?;
    if head_attempt.status.success() {
        return Ok(String::from_utf8_lossy(&head_attempt.stdout).into_owned());
    }

    let fallback = Command::new("git")
        .args(["-c", "core.quotePath=false", "diff"])
        .current_dir(root)
        .output()
        .context("failed to execute git diff")?;
    if !fallback.status.success() {
        let stderr = String::from_utf8_lossy(&fallback.stderr);
        bail!("git diff failed: {}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&fallback.stdout).into_owned())
}

fn collect_touched_files(raw_diff: &str) -> Vec<String> {
    let mut set = BTreeSet::new();
    for line in raw_diff.lines() {
        let Some(rest) = line.strip_prefix("diff --git ") else {
            continue;
        };
        let path = if let Some(idx) = rest.rfind("\" \"b/") {
            Some(rest[idx + 3..].trim_end_matches('"').to_owned())
        } else {
            rest.rsplit_once(" b/")
                .map(|(_, b)| b.trim_matches('"').to_owned())
        };
        if let Some(rel) = path
            && !is_noise_file(&rel)
        {
            set.insert(CodeGraph::normalize_rel_path(&rel));
        }
    }
    set.into_iter().collect()
}

fn print_affected_symbols(root: &Path, touched: &[String]) {
    if touched.is_empty() {
        return;
    }
    let bin_path = store::default_bin_path(root);
    if !bin_path.is_file() {
        return;
    }
    let Ok(graph) = store::load_binary(&bin_path) else {
        return;
    };

    for rel in touched {
        let Some(nids) = graph.file_nodes.get(rel) else {
            continue;
        };
        let symbols: Vec<&str> = nids
            .iter()
            .filter_map(|&nid| graph.get_node(nid))
            .filter(|n| n.kind == NodeKind::Symbol)
            .map(|n| n.label.as_str())
            .take(8)
            .collect();
        if !symbols.is_empty() {
            println!("# affected_symbols: {rel} -> {}", symbols.join(", "));
        }
    }
}
