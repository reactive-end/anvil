//! Heavy-file & noise-file Token Guard for `PreToolUse` read-only file reads.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;

use anvil_adapters::contract::{HookOutcome, InboxMessage, MessageLevel, NormalizedHookInput};
use anvil_adapters::inbox;
use anvil_core::diff_grep::is_noise_file;
use anvil_core::stats::{StatsEvent, record_event};
use serde_json::Value;

use super::payload::{extract_rel_file, strip_tool_namespace};

const HEAVY_FILE_LINE_THRESHOLD: usize = 800;
const BOUNDED_RANGE_KEYS: [&str; 6] = [
    "StartLine",
    "EndLine",
    "start_line",
    "end_line",
    "offset",
    "limit",
];

/// Evaluates `PreToolUse` file-reading tools without bounded line ranges.
/// Blocks noisy/generated files and emits an advisory `InboxMessage` for files > 800 lines.
pub fn check_read_token_guard(root: &Path, input: &NormalizedHookInput) -> Option<HookOutcome> {
    if !is_file_read_tool(input.tool_name.as_deref()) {
        return None;
    }
    if has_bounded_range(input.tool_input.as_ref()) {
        return None;
    }
    let rel = extract_rel_file(root, input.tool_input.as_ref())?;

    if is_noise_file(&rel) {
        let _ = record_event(root, StatsEvent::TokenGuard { blocked: true });
        return Some(HookOutcome::deny(format!(
            "Token Guard blocked unbounded read of noisy/generated file `{rel}`. Use bounded line ranges (`StartLine`/`EndLine` or `offset`/`limit`), `anvil grep`, or `graph_outline`/`graph_read_symbol`."
        )));
    }

    let abs = root.join(&rel);
    if abs.is_file()
        && let Some(line_count) = count_lines_if_heavy(&abs, HEAVY_FILE_LINE_THRESHOLD)
    {
        let _ = record_event(root, StatsEvent::TokenGuard { blocked: false });
        let msg = InboxMessage::new(
            "token-guard",
            MessageLevel::Warning,
            format!(
                "`{rel}` exceeds {HEAVY_FILE_LINE_THRESHOLD} lines ({line_count}+ lines). Prefer `graph_outline`, `graph_read_symbol`, or bounded line ranges to save tokens."
            ),
            Some(rel),
        );
        let _ = inbox::push_messages(root, std::slice::from_ref(&msg));
        return Some(HookOutcome::with_messages(vec![msg]));
    }
    None
}

fn is_file_read_tool(tool_name: Option<&str>) -> bool {
    let Some(bare) = strip_tool_namespace(tool_name) else {
        return false;
    };
    matches!(bare, "read_file" | "Read" | "view_file")
}

fn has_bounded_range(tool_input: Option<&Value>) -> bool {
    let Some(obj) = tool_input.and_then(Value::as_object) else {
        return false;
    };
    BOUNDED_RANGE_KEYS
        .iter()
        .any(|k| obj.get(*k).is_some_and(|v| !v.is_null()))
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
