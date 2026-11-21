//! `anvil graph`: build, incremental update, query, path, explain, report, and multi-repo merge.

use std::path::{Path, PathBuf};

use anyhow::Result;
use anvil_core::config::AnvilConfig;
use anvil_graph::merge::merge_graphs;
use anvil_graph::{CodeGraphEngine, store};

use crate::cli::subcmds::GraphSubcmd;

/// Executes `anvil graph <subcommand>`.
pub fn execute(root: &Path, sub: GraphSubcmd) -> Result<()> {
    let config = AnvilConfig::load_or_default(root)?;
    match sub {
        GraphSubcmd::Build {
            json,
            html,
            overlay,
        } => cmd_build(root, config, json, html.as_deref(), overlay.as_deref()),
        GraphSubcmd::Update => cmd_update(root, config),
        GraphSubcmd::Query { term, limit } => cmd_query(root, config, &term, limit),
        GraphSubcmd::Path { from, to } => cmd_path(root, config, &from, &to),
        GraphSubcmd::Explain { target } => cmd_explain(root, config, &target),
        GraphSubcmd::Report { html } => cmd_report(root, config, html.as_deref()),
        GraphSubcmd::Merge { dirs, out } => cmd_merge(root, &config, &dirs, out.as_deref()),
    }
}

fn cmd_build(
    root: &Path,
    config: AnvilConfig,
    json: bool,
    html: Option<&Path>,
    overlay: Option<&Path>,
) -> Result<()> {
    let mut engine = CodeGraphEngine::build(root, config)?;
    if let Some(overlay_path) = overlay {
        let stats = engine.apply_graphify_overlay(overlay_path)?;
        println!(
            "overlay applied: {} semantic nodes, {} semantic edges, {} hyperedges",
            stats.semantic_nodes_added, stats.semantic_edges_added, stats.hyperedges_linked
        );
    }
    let _ = engine.detect_communities();
    let bin = engine.save()?;
    println!(
        "graph built: {} nodes, {} edges -> {}",
        engine.graph.nodes.len(),
        engine.graph.edges.len(),
        bin.display()
    );
    if json {
        let json_path = engine.export_json()?;
        println!("exported JSON: {}", json_path.display());
    }
    if let Some(html_path) = html {
        engine.export_html(html_path)?;
        println!("exported HTML: {}", html_path.display());
    }
    Ok(())
}

fn cmd_update(root: &Path, config: AnvilConfig) -> Result<()> {
    let mut engine = CodeGraphEngine::load_or_build(root, config)?;
    let stats = engine.update_incremental()?;
    let bin = engine.save()?;
    println!(
        "graph updated: +{} added, ~{} modified, -{} removed, {} unchanged -> {}",
        stats.added,
        stats.modified,
        stats.removed,
        stats.unchanged,
        bin.display()
    );
    Ok(())
}

fn cmd_query(root: &Path, config: AnvilConfig, term: &str, limit: usize) -> Result<()> {
    let engine = CodeGraphEngine::load_or_build(root, config)?;
    let hits = engine.query(term, limit);
    if hits.is_empty() {
        println!("no graph hits for `{term}`");
        return Ok(());
    }
    for hit in hits {
        println!(
            "[{:?}] {} ({}:{}) score={:.2}",
            hit.kind, hit.label, hit.file, hit.line, hit.score
        );
    }
    Ok(())
}

fn cmd_path(root: &Path, config: AnvilConfig, from: &str, to: &str) -> Result<()> {
    let engine = CodeGraphEngine::load_or_build(root, config)?;
    let Some(res) = engine.shortest_path(from, to) else {
        println!("no directed path found from `{from}` to `{to}`");
        return Ok(());
    };
    println!("path {} -> {} ({} hops):", res.from, res.to, res.steps.len());
    for step in res.steps {
        println!("  {} --[{:?}@L{}]--> {}", step.from, step.kind, step.line, step.to);
    }
    Ok(())
}

fn cmd_explain(root: &Path, config: AnvilConfig, target: &str) -> Result<()> {
    let engine = CodeGraphEngine::load_or_build(root, config)?;
    let Some(exp) = engine.explain(target) else {
        println!("node `{target}` not found in graph");
        return Ok(());
    };
    println!(
        "{} ({}:{}) kind={:?} community={:?} god_node={}",
        exp.node.label, exp.node.file, exp.node.line, exp.node.kind, exp.community, exp.is_god_node
    );
    println!("incoming ({}):", exp.incoming.len());
    for edge in exp.incoming.iter().take(20) {
        println!("  <- [{:?}@L{}] {} ({})", edge.kind, edge.line, edge.node_id, edge.file);
    }
    println!("outgoing ({}):", exp.outgoing.len());
    for edge in exp.outgoing.iter().take(20) {
        println!("  -> [{:?}@L{}] {} ({})", edge.kind, edge.line, edge.node_id, edge.file);
    }
    Ok(())
}

fn cmd_report(root: &Path, config: AnvilConfig, html: Option<&Path>) -> Result<()> {
    let mut engine = CodeGraphEngine::load_or_build(root, config)?;
    let md = engine.report_markdown();
    println!("{md}");
    if let Some(html_path) = html {
        engine.export_html(html_path)?;
        println!("exported HTML: {}", html_path.display());
    }
    Ok(())
}

fn cmd_merge(
    root: &Path,
    config: &AnvilConfig,
    dirs: &[PathBuf],
    out: Option<&Path>,
) -> Result<()> {
    let targets: Vec<(String, PathBuf)> = if !dirs.is_empty() {
        dirs.iter()
            .map(|d| {
                let abs = if d.is_absolute() { d.clone() } else { root.join(d) };
                let name = d
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("repo")
                    .to_owned();
                (name, abs)
            })
            .collect()
    } else {
        config
            .subrepos
            .iter()
            .map(|s| (s.name.clone(), root.join(&s.path)))
            .collect()
    };

    let mut built = Vec::with_capacity(targets.len());
    for (name, dir) in targets {
        let sub_cfg = AnvilConfig::load_or_default(&dir).unwrap_or_else(|_| config.clone());
        let engine = CodeGraphEngine::build(&dir, sub_cfg)?;
        built.push((name, engine.graph));
    }

    let refs: Vec<(&str, &anvil_graph::model::CodeGraph)> =
        built.iter().map(|(n, g)| (n.as_str(), g)).collect();
    let merged = merge_graphs(&refs);
    let out_path = out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| store::default_bin_path(root));
    store::save_binary(&merged, &out_path)?;
    println!(
        "merged {} repo(s): {} nodes, {} edges -> {}",
        built.len(),
        merged.nodes.len(),
        merged.edges.len(),
        out_path.display()
    );
    Ok(())
}
