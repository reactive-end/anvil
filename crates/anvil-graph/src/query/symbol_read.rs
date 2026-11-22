//! Targeted AST-span symbol reader (`graph_read_symbol`) for token-efficient context loading.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use anvil_core::stats::estimate_tokens;
use serde::{Deserialize, Serialize};

use crate::model::{CodeGraph, GraphNode, NodeKind};
use crate::query::symbol_kind_str;

const MAX_SYMBOL_LINES: u32 = 250;

/// Exact symbol source slice returned by `read_symbol`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymbolSlice {
    pub file: String,
    pub symbol: String,
    pub kind: String,
    pub start_line: u32,
    pub end_line: u32,
    pub total_file_lines: u32,
    pub imports: Vec<String>,
    pub body: String,
    pub estimated_tokens_saved: u64,
}

/// Reads only the AST line span `start_line..=end_line` of `target` from `repo_root`.
pub fn read_symbol(
    graph: &CodeGraph,
    repo_root: &Path,
    target: &str,
    include_imports: bool,
) -> Result<Option<SymbolSlice>> {
    let Some(node) = find_symbol_node(graph, target.trim()) else {
        return Ok(None);
    };

    let abs = repo_root.join(&node.file);
    let content = fs::read_to_string(&abs)
        .with_context(|| format!("failed to read symbol source {}", abs.display()))?;
    let lines: Vec<&str> = content.lines().collect();
    let total_file_lines = u32::try_from(lines.len()).unwrap_or(u32::MAX);

    let start_line = node.line.max(1);
    let raw_end = node.end_line.max(start_line);
    let end_line = if total_file_lines == 0 {
        start_line
    } else {
        raw_end.min(total_file_lines)
    };
    let capped_end = end_line.min(start_line.saturating_add(MAX_SYMBOL_LINES - 1));

    let body = slice_body(&lines, start_line, capped_end, end_line);
    let imports = if include_imports {
        collect_top_imports(&lines, start_line, capped_end)
    } else {
        Vec::new()
    };

    let import_bytes: usize = imports.iter().map(|s| s.len() + 1).sum();
    let slice_bytes = body.len() + import_bytes;
    let estimated_tokens_saved =
        estimate_tokens(content.len()).saturating_sub(estimate_tokens(slice_bytes));

    Ok(Some(SymbolSlice {
        file: node.file.clone(),
        symbol: node.label.clone(),
        kind: symbol_kind_str(node.symbol_kind).to_owned(),
        start_line,
        end_line,
        total_file_lines,
        imports,
        body,
        estimated_tokens_saved,
    }))
}

fn slice_body(lines: &[&str], start_line: u32, capped_end: u32, full_end: u32) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let start_idx = (start_line.saturating_sub(1) as usize).min(lines.len().saturating_sub(1));
    let end_idx = (capped_end as usize).min(lines.len()).max(start_idx + 1);
    let mut body = lines[start_idx..end_idx].join("\n");
    if full_end > capped_end {
        let extra = full_end - capped_end;
        body.push_str(&format!("\n// ... truncated ({extra} more lines)"));
    }
    body
}

fn collect_top_imports(lines: &[&str], start_line: u32, capped_end: u32) -> Vec<String> {
    let mut imports = Vec::new();
    for (idx, line) in lines.iter().enumerate() {
        let line_no = (idx + 1) as u32;
        if line_no >= start_line && line_no <= capped_end {
            continue;
        }
        let trimmed = line.trim();
        let is_import = trimmed.starts_with("import ")
            || trimmed.starts_with("use ")
            || trimmed.starts_with("pub use ")
            || (trimmed.starts_with("from ") && trimmed.contains(" import "));
        if is_import {
            imports.push((*line).to_owned());
        }
    }
    imports
}

fn find_symbol_node<'a>(graph: &'a CodeGraph, target: &str) -> Option<&'a GraphNode> {
    if target.is_empty() {
        return None;
    }
    if let Some((_, node)) = graph.node_by_id(target)
        && node.kind == NodeKind::Symbol
    {
        return Some(node);
    }

    if let Some((raw_file, sym_part)) = target.strip_prefix("sym:").unwrap_or(target).split_once("::")
    {
        let norm_file = CodeGraph::normalize_rel_path(raw_file);
        let canonical = CodeGraph::symbol_node_id(&norm_file, sym_part);
        if let Some((_, node)) = graph.node_by_id(&canonical) {
            return Some(node);
        }
        if let Some(nids) = graph.file_nodes.get(&norm_file) {
            for &nid in nids {
                if let Some(node) = graph.get_node(nid)
                    && node.kind == NodeKind::Symbol
                    && (node.label == sym_part || node.id.ends_with(&format!("::{sym_part}")))
                {
                    return Some(node);
                }
            }
        }
    }

    if let Some(nids) = graph.symbol_index.get(target) {
        let mut candidates: Vec<&GraphNode> = nids
            .iter()
            .filter_map(|&nid| graph.get_node(nid))
            .filter(|n| n.kind == NodeKind::Symbol)
            .collect();
        candidates.sort_by(|a, b| {
            b.exported
                .cmp(&a.exported)
                .then_with(|| a.file.cmp(&b.file))
                .then_with(|| a.line.cmp(&b.line))
        });
        if let Some(best) = candidates.into_iter().next() {
            return Some(best);
        }
    }

    graph.nodes.iter().find(|n| {
        !n.is_tombstone() && n.kind == NodeKind::Symbol && n.label.eq_ignore_ascii_case(target)
    })
}
