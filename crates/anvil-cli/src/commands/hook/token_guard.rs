//! Heavy-file & noise-file Token Guard for `PreToolUse` read-only file reads.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use anvil_adapters::contract::{HookOutcome, InboxMessage, MessageLevel, NormalizedHookInput};
use anvil_adapters::inbox;
use anvil_core::config::AnvilConfig;
use anvil_core::diff_grep::is_noise_file;
use anvil_core::stats::{StatsEvent, record_event};
use anvil_graph::CodeGraphEngine;
use serde_json::Value;

use super::payload::{extract_rel_file, strip_tool_namespace};
use crate::daemon::protocol::{DaemonRequest, DaemonResponse};
use crate::daemon::try_request;

const HEAVY_FILE_LINE_THRESHOLD: usize = 800;
const MAX_SAFE_READ_WIDTH: usize = 400;
const MAX_NOISE_READ_WIDTH: usize = 200;

/// Evaluates `PreToolUse` file-reading tools, enforcing noise guards and outline-guided heavy reads.
pub fn check_read_token_guard(root: &Path, input: &NormalizedHookInput) -> Option<HookOutcome> {
    if !is_file_read_tool(input.tool_name.as_deref()) {
        return None;
    }
    let rel = extract_rel_file(root, input.tool_input.as_ref())?;
    let range_width = compute_range_width(input.tool_input.as_ref());

    if is_noise_file(&rel) {
        if range_width.is_some_and(|w| w <= MAX_NOISE_READ_WIDTH) {
            return None;
        }
        let _ = record_event(root, StatsEvent::TokenGuard { blocked: true });
        return Some(HookOutcome::deny(format!(
            "Token Guard blocked read of noisy/generated file `{rel}` (max bounded width {MAX_NOISE_READ_WIDTH} lines). Use `StartLine`/`EndLine`, `anvil grep`, or `graph_outline`/`graph_read_symbol`."
        )));
    }

    if range_width.is_some_and(|w| w <= MAX_SAFE_READ_WIDTH) {
        return None;
    }

    let abs = root.join(&rel);
    if abs.is_file()
        && let Some(line_count) = count_lines_if_heavy(&abs, HEAVY_FILE_LINE_THRESHOLD)
    {
        return deny_heavy_file_with_outline(root, &rel, line_count);
    }
    None
}

fn deny_heavy_file_with_outline(root: &Path, rel: &str, line_count: usize) -> Option<HookOutcome> {
    let _ = record_event(root, StatsEvent::TokenGuard { blocked: true });
    let outline_text = fetch_compact_outline(root, rel);
    let warn = InboxMessage::new(
        "token-guard",
        MessageLevel::Warning,
        format!("`{rel}` exceeds {HEAVY_FILE_LINE_THRESHOLD} lines ({line_count}+ lines)."),
        Some(rel.to_owned()),
    );
    let _ = inbox::push_messages(root, std::slice::from_ref(&warn));

    let reason = if outline_text.is_empty() {
        format!(
            "Token Guard blocked unbounded read of heavy file `{rel}` ({line_count}+ lines > {HEAVY_FILE_LINE_THRESHOLD} lines). Use bounded `StartLine`/`EndLine` (<= {MAX_SAFE_READ_WIDTH} lines), `anvil grep`, or `graph_read_symbol`."
        )
    } else {
        format!(
            "Token Guard blocked unbounded read of heavy file `{rel}` ({line_count}+ lines > {HEAVY_FILE_LINE_THRESHOLD} lines). Read a specific symbol span via `StartLine`/`EndLine` (<= {MAX_SAFE_READ_WIDTH}) or `graph_read_symbol`.\nOutline:\n{outline_text}"
        )
    };
    Some(HookOutcome::deny(reason))
}

fn fetch_compact_outline(root: &Path, rel: &str) -> String {
    if let Some(DaemonResponse::OutlineResult { outline }) = try_request(
        root,
        DaemonRequest::Outline {
            target: Some(rel.to_owned()),
            max_tokens: 450,
        },
    ) {
        return format_outline_lines(&outline);
    }
    if anvil_graph::store::default_bin_path(root).is_file()
        && let Ok(config) = AnvilConfig::load_or_default(root)
        && let Ok(engine) = CodeGraphEngine::load_cached(root, config)
    {
        return format_outline_lines(&engine.outline(Some(rel), 450));
    }
    String::new()
}

fn format_outline_lines(outline: &anvil_graph::query::RepoOutline) -> String {
    let mut lines = Vec::new();
    for file in outline.files.iter().take(2) {
        for sym in file.symbols.iter().take(18) {
            let sig = sym.signature.as_deref().unwrap_or(&sym.name);
            lines.push(format!("  L{}: {} {}", sym.line, sym.kind, sig));
        }
    }
    lines.join("\n")
}

fn is_file_read_tool(tool_name: Option<&str>) -> bool {
    let Some(bare) = strip_tool_namespace(tool_name) else {
        return false;
    };
    matches!(bare, "read_file" | "Read" | "view_file")
}

fn compute_range_width(tool_input: Option<&Value>) -> Option<usize> {
    let obj = tool_input?.as_object()?;
    let start = opt_usize(obj, &["StartLine", "start_line", "offset"]);
    let end = opt_usize(obj, &["EndLine", "end_line"]);
    let limit = opt_usize(obj, &["limit"]);
    if let (Some(s), Some(e)) = (start, end) {
        return Some(e.saturating_sub(s).saturating_add(1));
    }
    if let Some(l) = limit {
        return Some(l.max(1));
    }
    None
}

fn opt_usize(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<usize> {
    keys.iter()
        .find_map(|k| obj.get(*k).and_then(Value::as_u64))
        .and_then(|v| usize::try_from(v).ok())
}

fn count_lines_if_heavy(path: &Path, threshold: usize) -> Option<usize> {
    let file = File::open(path).ok()?;
    let mut reader = BufReader::new(file);
    let mut buf = [0u8; 8192];
    let mut newlines = 0usize;
    let mut last_byte = None;
    let mut saw_any = false;

    loop {
        let n = reader.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        saw_any = true;
        last_byte = Some(buf[n - 1]);
        newlines += buf[..n].iter().filter(|&&b| b == b'\n').count();
        if newlines > threshold {
            return Some(newlines);
        }
    }

    let total_lines = if !saw_any {
        0
    } else if last_byte == Some(b'\n') {
        newlines
    } else {
        newlines + 1
    };
    (total_lines > threshold).then_some(total_lines)
}
