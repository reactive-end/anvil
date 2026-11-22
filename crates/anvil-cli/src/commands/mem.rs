//! `anvil mem`: persistent FTS5 memory CLI with progressive disclosure and staleness check.

use std::path::Path;

use anyhow::Result;
use anvil_core::config::AnvilConfig;
use anvil_graph::CodeGraphEngine;
use anvil_memory::{MemoryEngine, MemoryKind, ObservationInput};
use anvil_tasks::{resolve_author, resolve_author_fast};

use crate::cli::subcmds::MemSubcmd;

/// Executes `anvil mem <subcommand>`.
pub fn execute(root: &Path, sub: MemSubcmd) -> Result<()> {
    let author = match &sub {
        MemSubcmd::Save { .. } => resolve_author(root),
        _ => resolve_author_fast(root),
    }
    .unwrap_or_else(|_| "cli".to_owned());
    let mut mem = MemoryEngine::open_without_sync(root, author)?;

    match sub {
        save_sub @ MemSubcmd::Save { .. } => cmd_save(root, &mut mem, save_sub)?,
        MemSubcmd::Search { query, kind, limit } => {
            let _ = mem.sync();
            cmd_search(&mem, &query, kind.as_deref(), limit)?;
        }
        MemSubcmd::Timeline { id_or_topic, before, after } => {
            let _ = mem.sync();
            cmd_timeline(&mem, &id_or_topic, before, after)?;
        }
        MemSubcmd::Get { id } => {
            let _ = mem.sync();
            cmd_get(&mem, &id)?;
        }
        MemSubcmd::Context { max_tokens } => {
            let _ = mem.sync();
            cmd_context(&mem, max_tokens)?;
        }
        MemSubcmd::Sync => {
            let stats = mem.sync()?;
            println!(
                "memory sync: scanned={} ingested={} applied={}",
                stats.files_scanned, stats.files_ingested, stats.records_applied
            );
        }
        MemSubcmd::CheckStale => {
            let _ = mem.sync();
            cmd_check_stale(root, &mut mem)?;
        }
        MemSubcmd::Cleanup => {
            let deleted = mem.cleanup_expired_sessions()?;
            println!("cleaned up {deleted} expired session observation(s)");
        }
    }
    Ok(())
}

fn cmd_save(root: &Path, mem: &mut MemoryEngine, sub: MemSubcmd) -> Result<()> {
    let MemSubcmd::Save {
        kind,
        title,
        what,
        why,
        location,
        learned,
        topic_key,
        files,
        symbols,
        ttl,
    } = sub
    else {
        return Ok(());
    };
    let resolved_symbols = if symbols.is_empty() && !files.is_empty() {
        infer_symbols_from_graph(root, &files)
    } else {
        symbols
    };
    let obs = mem.save(ObservationInput {
        topic_key,
        kind: kind.parse()?,
        title,
        what,
        why,
        location,
        learned,
        related_files: files,
        related_symbols: resolved_symbols,
        ttl_seconds: ttl,
    })?;
    println!(
        "saved memory id={} topic_key={} kind={} symbols={}",
        obs.id,
        obs.topic_key,
        obs.kind,
        obs.related_symbols.len()
    );
    Ok(())
}

fn infer_symbols_from_graph(root: &Path, files: &[String]) -> Vec<String> {
    let Ok(cfg) = AnvilConfig::load_or_default(root) else {
        return Vec::new();
    };
    let Ok(engine) = CodeGraphEngine::load_cached(root, cfg) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for file in files {
        let norm = anvil_graph::model::CodeGraph::normalize_rel_path(file);
        if let Some(nids) = engine.graph.file_nodes.get(&norm) {
            for &nid in nids {
                if let Some(node) = engine.graph.get_node(nid)
                    && node.kind == anvil_graph::model::NodeKind::Symbol
                    && node.exported
                    && !out.iter().any(|s| s == &node.label)
                {
                    out.push(node.label.clone());
                    if out.len() >= 8 {
                        return out;
                    }
                }
            }
        }
    }
    out
}

fn cmd_search(mem: &MemoryEngine, query: &str, kind: Option<&str>, limit: usize) -> Result<()> {
    let parsed_kind = kind.map(str::parse::<MemoryKind>).transpose()?;
    let hits = mem.search(query, parsed_kind, limit)?;
    if hits.is_empty() {
        println!("no memory hits for `{query}`");
        return Ok(());
    }
    for hit in hits {
        println!(
            "[{}] {} (id={} topic={} freshness={:?}) — {}",
            hit.kind, hit.title, hit.id, hit.topic_key, hit.freshness, hit.snippet
        );
    }
    Ok(())
}

fn cmd_timeline(mem: &MemoryEngine, id_or_topic: &str, before: usize, after: usize) -> Result<()> {
    let entries = mem.timeline(id_or_topic, before, after)?;
    if entries.is_empty() {
        println!("no timeline entries for `{id_or_topic}`");
        return Ok(());
    }
    for e in entries {
        let marker = if e.is_anchor { "*" } else { " " };
        println!("{marker} [{}] {} ({}) freshness={:?}", e.kind, e.title, e.id, e.freshness);
    }
    Ok(())
}

fn cmd_get(mem: &MemoryEngine, id: &str) -> Result<()> {
    let obs = match mem.get(id)? {
        Some(found) => Some(found),
        None => mem.get_by_topic(id)?,
    };
    let Some(obs) = obs else {
        println!("memory `{id}` not found");
        return Ok(());
    };
    println!("{}", serde_json::to_string_pretty(&obs)?);
    Ok(())
}

fn cmd_context(mem: &MemoryEngine, max_tokens: usize) -> Result<()> {
    let hits = mem.session_context(max_tokens)?;
    println!("session context ({} hits, budget={max_tokens} tokens):", hits.len());
    for hit in hits {
        println!("- [{}][{:?}] {} ({}) — {}", hit.kind, hit.freshness, hit.title, hit.id, hit.snippet);
    }
    Ok(())
}

fn cmd_check_stale(root: &Path, mem: &mut MemoryEngine) -> Result<()> {
    let cfg = AnvilConfig::load_or_default(root)?;
    let engine = CodeGraphEngine::load_or_build(root, cfg)?;
    let updates = mem.check_staleness(&engine)?;
    println!("staleness check: {} transition(s)", updates.len());
    for u in updates {
        println!(
            "  {} ({}): {:?} -> {:?} ({})",
            u.topic_key,
            u.id,
            u.previous,
            u.current,
            u.reason.as_deref().unwrap_or("ok")
        );
    }
    Ok(())
}
