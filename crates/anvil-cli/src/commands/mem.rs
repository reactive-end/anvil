//! `anvil mem`: persistent FTS5 memory CLI with progressive disclosure and staleness check.

use std::path::Path;

use anyhow::Result;
use anvil_core::config::AnvilConfig;
use anvil_graph::CodeGraphEngine;
use anvil_memory::{MemoryEngine, MemoryKind, ObservationInput};
use anvil_tasks::resolve_author;

use crate::cli::subcmds::MemSubcmd;

/// Executes `anvil mem <subcommand>`.
pub fn execute(root: &Path, sub: MemSubcmd) -> Result<()> {
    let author = resolve_author(root).unwrap_or_else(|_| "cli".to_owned());
    let mut mem = MemoryEngine::open(root, author)?;

    match sub {
        MemSubcmd::Save {
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
        } => {
            let input = ObservationInput {
                topic_key,
                kind: kind.parse()?,
                title,
                what,
                why,
                location,
                learned,
                related_files: files,
                related_symbols: symbols,
                ttl_seconds: ttl,
            };
            let obs = mem.save(input)?;
            println!("saved memory id={} topic_key={} kind={}", obs.id, obs.topic_key, obs.kind);
        }
        MemSubcmd::Search { query, kind, limit } => cmd_search(&mem, &query, kind.as_deref(), limit)?,
        MemSubcmd::Timeline { id_or_topic, before, after } => cmd_timeline(&mem, &id_or_topic, before, after)?,
        MemSubcmd::Get { id } => cmd_get(&mem, &id)?,
        MemSubcmd::Context { max_tokens } => cmd_context(&mem, max_tokens)?,
        MemSubcmd::Sync => {
            let stats = mem.sync()?;
            println!(
                "memory sync: scanned={} ingested={} applied={}",
                stats.files_scanned, stats.files_ingested, stats.records_applied
            );
        }
        MemSubcmd::CheckStale => cmd_check_stale(root, &mut mem)?,
        MemSubcmd::Cleanup => {
            let deleted = mem.cleanup_expired_sessions()?;
            println!("cleaned up {deleted} expired session observation(s)");
        }
    }
    Ok(())
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
