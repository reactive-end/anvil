//! `anvil context pack`: minimal subagent context packet from `anvil-graph` + `anvil-memory`.

use std::fmt::Write as _;
use std::path::Path;

use anyhow::Result;
use anvil_core::config::AnvilConfig;
use anvil_graph::CodeGraphEngine;
use anvil_graph::model::CodeGraph;
use anvil_memory::MemoryEngine;
use anvil_tasks::resolve_author;
use rustc_hash::FxHashSet;

use crate::cli::subcmds::ContextSubcmd;

/// Executes `anvil context pack <targets...> [--max-tokens N]`.
pub fn execute(root: &Path, sub: ContextSubcmd) -> Result<()> {
    match sub {
        ContextSubcmd::Pack {
            targets,
            max_tokens,
        } => build_and_print_packet(root, &targets, max_tokens),
    }
}

fn build_and_print_packet(root: &Path, targets: &[String], max_tokens: usize) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    let engine = CodeGraphEngine::load_or_build(root, config)?;
    let author = resolve_author(root).unwrap_or_else(|_| "cli".to_owned());
    let mem = MemoryEngine::open(root, author)?;

    let target_files = resolve_target_files(&engine, targets);
    let blast = engine.blast_radius(&target_files, 1);
    let memories = collect_relevant_memories(&mem, targets)?;

    let mut packet = String::with_capacity(max_tokens.saturating_mul(4).min(8192));
    append_scope_section(&mut packet, targets, &target_files, &blast.impacted_files);
    append_neighbors_section(&mut packet, &engine, targets);
    append_memory_section(&mut packet, &memories);
    append_done_section(&mut packet, &blast.related_tests);

    let clamped = clamp_to_token_budget(&packet, max_tokens);
    println!("{clamped}");
    Ok(())
}

fn resolve_target_files(engine: &CodeGraphEngine, targets: &[String]) -> Vec<String> {
    let mut files = Vec::new();
    let mut seen = FxHashSet::default();
    for target in targets {
        let norm = CodeGraph::normalize_rel_path(target);
        if engine.graph.file_hashes.contains_key(&norm) && seen.insert(norm.clone()) {
            files.push(norm);
            continue;
        }
        if let Some(exp) = engine.explain(target)
            && !exp.node.file.is_empty()
            && seen.insert(exp.node.file.clone())
        {
            files.push(exp.node.file);
        }
    }
    files
}

fn collect_relevant_memories(
    mem: &MemoryEngine,
    targets: &[String],
) -> Result<Vec<(String, String, String)>> {
    let mut seen = FxHashSet::default();
    let mut out = Vec::new();
    for target in targets {
        for hit in mem.search(target, None, 3)? {
            if seen.insert(hit.id.clone()) {
                out.push((hit.kind.to_string(), hit.title, hit.snippet));
            }
        }
    }
    Ok(out)
}

fn append_scope_section(
    out: &mut String,
    targets: &[String],
    files: &[String],
    impacted: &[anvil_graph::impact::ImpactedItem],
) {
    let _ = writeln!(out, "# Minimal Context Packet");
    let _ = writeln!(out, "## Ownership & Scope");
    let _ = writeln!(out, "- Targets: {}", targets.join(", "));
    let _ = writeln!(
        out,
        "- Resolved files: {}",
        if files.is_empty() { "none".to_owned() } else { files.join(", ") }
    );
    if !impacted.is_empty() {
        let list: Vec<&str> = impacted.iter().take(8).map(|i| i.file.as_str()).collect();
        let _ = writeln!(out, "- Direct consumers (d=1): {}", list.join(", "));
    }
}

fn append_neighbors_section(out: &mut String, engine: &CodeGraphEngine, targets: &[String]) {
    let _ = writeln!(out, "\n## Relevant Symbols & Edges");
    for target in targets.iter().take(5) {
        if let Some(exp) = engine.explain(target) {
            let _ = writeln!(
                out,
                "- `{}` ({}:{}) in={} out={}",
                exp.node.label,
                exp.node.file,
                exp.node.line,
                exp.incoming.len(),
                exp.outgoing.len()
            );
            for edge in exp.incoming.iter().take(4) {
                let _ = writeln!(out, "  <- {:?} {} ({})", edge.kind, edge.node_id, edge.file);
            }
            for edge in exp.outgoing.iter().take(4) {
                let _ = writeln!(out, "  -> {:?} {} ({})", edge.kind, edge.node_id, edge.file);
            }
        }
    }
}

fn append_memory_section(out: &mut String, memories: &[(String, String, String)]) {
    let _ = writeln!(out, "\n## Linked Memories");
    if memories.is_empty() {
        let _ = writeln!(out, "- None");
        return;
    }
    for (kind, title, snippet) in memories.iter().take(6) {
        let _ = writeln!(out, "- [{kind}] {title}: {snippet}");
    }
}

fn append_done_section(out: &mut String, related_tests: &[String]) {
    let _ = writeln!(out, "\n## Done Criteria");
    if related_tests.is_empty() {
        let _ = writeln!(out, "- Verify with `anvil check`");
    } else {
        let _ = writeln!(
            out,
            "- Verify with `anvil check` and related tests: {}",
            related_tests.iter().take(6).cloned().collect::<Vec<_>>().join(", ")
        );
    }
}

fn clamp_to_token_budget(text: &str, max_tokens: usize) -> String {
    let char_budget = max_tokens.saturating_mul(4);
    if text.len() <= char_budget {
        return text.to_owned();
    }
    let mut clipped: String = text.chars().take(char_budget).collect();
    clipped.push_str("\n... [context truncated to max_tokens]");
    clipped
}
