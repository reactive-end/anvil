//! Targeted AST symbol reading (`read-symbol`) and signature outline (`outline`) commands.

use std::path::Path;

use anyhow::Result;
use anvil_core::config::AnvilConfig;
use anvil_core::stats::{StatsEvent, record_event};
use anvil_graph::CodeGraphEngine;
use anvil_graph::query::{RepoOutline, SymbolSlice};

use crate::daemon::protocol::{DaemonRequest, DaemonResponse};
use crate::daemon::try_request;

/// Executes `anvil graph read-symbol <symbol> [--no-imports]`.
pub fn cmd_read_symbol(
    root: &Path,
    config: AnvilConfig,
    symbol: &str,
    include_imports: bool,
) -> Result<()> {
    let slice = match try_request(
        root,
        DaemonRequest::ReadSymbol {
            symbol: symbol.to_owned(),
            include_imports,
        },
    ) {
        Some(DaemonResponse::SymbolSliceResult { slice }) => slice,
        _ => {
            let engine = CodeGraphEngine::load_cached(root, config)?;
            engine.read_symbol(symbol, include_imports)?
        }
    };

    let Some(slice) = slice else {
        println!("symbol `{symbol}` not found in graph");
        return Ok(());
    };

    if slice.estimated_tokens_saved > 0 {
        let _ = record_event(
            root,
            StatsEvent::SymbolReadSaved {
                tokens_saved: slice.estimated_tokens_saved,
            },
        );
    }
    print_symbol_slice(&slice);
    Ok(())
}

/// Executes `anvil graph outline [target] [--max-tokens N]`.
pub fn cmd_outline(
    root: &Path,
    config: AnvilConfig,
    target: Option<&str>,
    max_tokens: usize,
) -> Result<()> {
    let outline = match try_request(
        root,
        DaemonRequest::Outline {
            target: target.map(str::to_owned),
            max_tokens,
        },
    ) {
        Some(DaemonResponse::OutlineResult { outline }) => outline,
        _ => {
            let engine = CodeGraphEngine::load_cached(root, config)?;
            engine.outline(target, max_tokens)
        }
    };

    if outline.estimated_tokens_saved > 0 {
        let _ = record_event(
            root,
            StatsEvent::OutlineSaved {
                tokens_saved: outline.estimated_tokens_saved,
            },
        );
    }
    print_outline(&outline);
    Ok(())
}

fn print_symbol_slice(slice: &SymbolSlice) {
    println!(
        "{} ({}) {}:L{}..=L{} (total={} lines, tokens_saved={})",
        slice.symbol,
        slice.kind,
        slice.file,
        slice.start_line,
        slice.end_line,
        slice.total_file_lines,
        slice.estimated_tokens_saved
    );
    if !slice.imports.is_empty() {
        println!("imports:");
        for imp in &slice.imports {
            println!("  {imp}");
        }
    }
    println!("{}", slice.body);
}

fn print_outline(outline: &RepoOutline) {
    for file in &outline.files {
        println!("{}:", file.file);
        for route in &file.routes {
            println!("  route: {route}");
        }
        for sym in &file.symbols {
            let sig = sym.signature.as_deref().unwrap_or(&sym.name);
            println!("  [{} L{}..=L{}] {}", sym.kind, sym.line, sym.end_line, sig);
        }
    }
    println!(
        "outline: {}/{} files, truncated={}, tokens_saved={}",
        outline.files.len(),
        outline.files_total,
        outline.truncated,
        outline.estimated_tokens_saved
    );
}
