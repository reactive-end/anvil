//! Heavy-file, binary/media, and noise-file Token Guard for `PreToolUse` file reads.

use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::Path;

use anvil_adapters::contract::{AdapterKind, HookOutcome, NormalizedHookInput};
use anvil_core::config::{AnvilConfig, TokenGuardConfig};
use anvil_core::diff_grep::is_noise_file;
use anvil_core::stats::{StatsEvent, estimate_tokens, record_event};
use anvil_graph::CodeGraphEngine;
use serde_json::Value;

use super::payload::{
    extract_rel_file, extract_target_file_path, is_within_root, strip_tool_namespace,
};
use super::token_guard_media::{
    FileKind, classify_file_kind, format_bytes_human, parse_pdf_page_count,
};
use crate::daemon::protocol::{DaemonRequest, DaemonResponse};
use crate::daemon::try_request;

/// Evaluates `PreToolUse` file-reading tools across text, PDF, media, binary, and noise files.
pub fn check_read_token_guard(
    root: &Path,
    config: &AnvilConfig,
    input: &NormalizedHookInput,
) -> Option<HookOutcome> {
    if !is_file_read_tool(input.tool_name.as_deref()) {
        return None;
    }
    let abs = extract_target_file_path(input.tool_input.as_ref(), root)?;
    let inside_root = is_within_root(root, &abs);
    let rel = extract_rel_file(root, input.tool_input.as_ref())
        .unwrap_or_else(|| abs.to_string_lossy().replace('\\', "/"));
    let tg = &config.token_guard;
    let kind = classify_file_kind(&abs);

    if inside_root && is_blocked_noise_target(&rel, kind) {
        return check_noise_read(root, &rel, kind, input, tg);
    }

    match kind {
        FileKind::Pdf => check_pdf_read(root, &rel, &abs, input, tg),
        FileKind::Image | FileKind::AudioVideo | FileKind::OtherBinary => {
            check_binary_read(root, &rel, &abs, kind, tg)
        }
        FileKind::Text => check_text_read(root, &rel, &abs, input, tg),
    }
}

fn is_blocked_noise_target(rel: &str, kind: FileKind) -> bool {
    if !is_noise_file(rel) {
        return false;
    }
    let norm = rel.trim().replace('\\', "/").to_ascii_lowercase();
    if matches!(kind, FileKind::Pdf | FileKind::Image | FileKind::AudioVideo)
        && (norm == ".harness/.state/qa" || norm.starts_with(".harness/.state/qa/"))
    {
        return false;
    }
    if !matches!(kind, FileKind::Pdf | FileKind::Image | FileKind::AudioVideo) {
        return true;
    }
    norm == ".harness/.state"
        || norm.starts_with(".harness/.state/")
        || norm.split('/').any(|seg| {
            matches!(seg, "node_modules" | "target" | "dist" | ".next" | ".godot" | "vendor")
        })
}

fn check_noise_read(
    root: &Path,
    rel: &str,
    kind: FileKind,
    input: &NormalizedHookInput,
    tg: &TokenGuardConfig,
) -> Option<HookOutcome> {
    if kind == FileKind::Text
        && compute_range_width(input.tool_input.as_ref())
            .is_some_and(|w| w <= tg.max_noise_read_lines)
    {
        return None;
    }
    if kind == FileKind::Text {
        record_text_block(root, rel);
        let hint = range_param_hint(input.adapter, tg.max_noise_read_lines);
        return Some(HookOutcome::deny(format!(
            "Token Guard blocked read of noisy/generated file `{rel}` (max bounded width {} lines). Use {hint}, `anvil grep`, or `graph_outline`/`graph_read_symbol`.",
            tg.max_noise_read_lines
        )));
    }
    record_block(root, 0);
    Some(HookOutcome::deny(format!(
        "Token Guard blocked read of generated/binary noise file `{rel}`. Use `anvil grep` or inspect project sources instead."
    )))
}

fn check_pdf_read(
    root: &Path,
    rel: &str,
    abs: &Path,
    input: &NormalizedHookInput,
    tg: &TokenGuardConfig,
) -> Option<HookOutcome> {
    if let Some(pages_spec) =
        input.tool_input.as_ref().and_then(|v| v.get("pages")).and_then(Value::as_str)
        && let Some(page_count) = parse_pdf_page_count(pages_spec)
    {
        if page_count <= tg.max_pdf_pages {
            return None;
        }
        record_block(root, 0);
        return Some(HookOutcome::deny(format!(
            "Token Guard blocked PDF read of `{rel}` ({page_count} pages > {} max pages per read). Request a smaller range with `pages` (e.g. `pages=\"1-5\"`) or adjust `token_guard.max_pdf_pages` in `.harness/config.json`.",
            tg.max_pdf_pages
        )));
    }

    let file_len = abs.is_file().then(|| fs::metadata(abs).ok()).flatten()?.len();
    if file_len <= tg.max_pdf_bytes {
        return None;
    }
    record_block(root, 0);
    let guidance = match input.adapter {
        AdapterKind::ClaudeCode => format!(
            "Pass a bounded page range with `pages` (e.g. `pages=\"1-5\"`, <= {} pages) or adjust `token_guard.max_pdf_bytes` in `.harness/config.json`.",
            tg.max_pdf_pages
        ),
        _ => "Split or extract only the needed pages, or adjust `token_guard.max_pdf_bytes` in `.harness/config.json`.".to_owned(),
    };
    Some(HookOutcome::deny(format!(
        "Token Guard blocked unbounded read of heavy PDF `{rel}` ({} > {} limit). {guidance}",
        format_bytes_human(file_len),
        format_bytes_human(tg.max_pdf_bytes),
    )))
}

fn check_binary_read(
    root: &Path,
    rel: &str,
    abs: &Path,
    kind: FileKind,
    tg: &TokenGuardConfig,
) -> Option<HookOutcome> {
    let file_len = abs.is_file().then(|| fs::metadata(abs).ok()).flatten()?.len();
    if file_len <= tg.max_binary_bytes {
        return None;
    }
    record_block(root, 0);
    let (label, tip) = match kind {
        FileKind::Image => (
            "image",
            "Resize or compress the image, or adjust `token_guard.max_binary_bytes` in `.harness/config.json`.",
        ),
        FileKind::AudioVideo => (
            "media file",
            "Trim or downsample the media clip, or adjust `token_guard.max_binary_bytes` in `.harness/config.json`.",
        ),
        _ => (
            "binary file",
            "Inspect it with targeted metadata/CLI tools instead of reading the full file, or adjust `token_guard.max_binary_bytes` in `.harness/config.json`.",
        ),
    };
    Some(HookOutcome::deny(format!(
        "Token Guard blocked read of heavy {label} `{rel}` ({} > {} limit). {tip}",
        format_bytes_human(file_len),
        format_bytes_human(tg.max_binary_bytes),
    )))
}

fn check_text_read(
    root: &Path,
    rel: &str,
    abs: &Path,
    input: &NormalizedHookInput,
    tg: &TokenGuardConfig,
) -> Option<HookOutcome> {
    if compute_range_width(input.tool_input.as_ref()).is_some_and(|w| w <= tg.max_read_lines) {
        return None;
    }
    if abs.is_file()
        && let Some(line_count) = count_lines_if_heavy(abs, tg.max_text_lines)
    {
        return deny_heavy_file_with_outline(root, rel, line_count, input.adapter, tg);
    }
    None
}

fn deny_heavy_file_with_outline(
    root: &Path,
    rel: &str,
    line_count: usize,
    adapter: AdapterKind,
    tg: &TokenGuardConfig,
) -> Option<HookOutcome> {
    record_text_block(root, rel);
    let outline_text = fetch_compact_outline(root, rel);
    let hint = range_param_hint(adapter, tg.max_read_lines);
    let max_text = tg.max_text_lines;

    let reason = if outline_text.is_empty() {
        format!(
            "Token Guard blocked unbounded read of heavy text file `{rel}` ({line_count}+ lines > {max_text} lines). Use bounded {hint}, `anvil grep`, or `graph_read_symbol`."
        )
    } else {
        format!(
            "Token Guard blocked unbounded read of heavy text file `{rel}` ({line_count}+ lines > {max_text} lines). Read a specific symbol span via {hint} or `graph_read_symbol`.\nOutline:\n{outline_text}"
        )
    };
    Some(HookOutcome::deny(reason))
}

fn range_param_hint(adapter: AdapterKind, max_lines: usize) -> String {
    match adapter {
        AdapterKind::ClaudeCode => format!("`offset`/`limit` (`limit` <= {max_lines})"),
        _ => format!("`StartLine`/`EndLine` (<= {max_lines} lines)"),
    }
}

/// Records a denied read together with the tokens the file would have cost.
pub(super) fn record_block(root: &Path, tokens_avoided: u64) {
    if root.join(".harness").is_dir() {
        let _ = record_event(root, StatsEvent::TokenGuard { blocked: true, tokens_avoided });
    }
}

/// Estimates text cost only; media and binary formats are not billed as bytes / 4.
fn record_text_block(root: &Path, rel: &str) {
    let bytes = fs::metadata(root.join(rel)).map_or(0, |m| m.len());
    record_block(root, estimate_tokens(usize::try_from(bytes).unwrap_or(usize::MAX)));
}

fn fetch_compact_outline(root: &Path, rel: &str) -> String {
    if let Some(DaemonResponse::OutlineResult { outline }) =
        try_request(root, DaemonRequest::Outline { target: Some(rel.to_owned()), max_tokens: 450 })
    {
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
    match (start, end) {
        (Some(s), Some(e)) => Some(e.saturating_sub(s).saturating_add(1)),
        (None, Some(e)) => Some(e.max(1)),
        _ => limit.map(|l| l.max(1)),
    }
}

fn opt_usize(obj: &serde_json::Map<String, Value>, keys: &[&str]) -> Option<usize> {
    keys.iter()
        .find_map(|k| obj.get(*k).and_then(Value::as_u64))
        .and_then(|v| usize::try_from(v).ok())
}

pub(super) fn count_lines_if_heavy(path: &Path, threshold: usize) -> Option<usize> {
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
