//! Token Guard for whole-file dumps issued through the shell (`cat`, `type`, `Get-Content`).

use std::fs;
use std::path::{Path, PathBuf};

use anvil_adapters::contract::{HookOutcome, NormalizedHookInput};
use anvil_core::config::AnvilConfig;
use anvil_core::diff_grep::is_noise_file;
use anvil_core::stats::estimate_tokens;

use super::payload::{extract_command_field, extract_rel_file, is_within_root};
use super::shell::expand::{Invocation, expand};
use super::token_guard::{count_lines_if_heavy, record_block};

const DUMPERS: &[&str] = &["cat", "type", "get-content", "gc", "bat", "less", "more", "nl"];
const BOUNDING_FLAGS: &[&str] = &["-totalcount", "-head", "-tail", "-first", "-last", "-readcount"];

/// Denies commands that would print a noise file or a heavy text file straight into the context.
///
/// Output that is piped or redirected is left alone because the agent never sees it whole.
pub fn check_shell_token_guard(
    root: &Path,
    config: &AnvilConfig,
    input: &NormalizedHookInput,
) -> Option<HookOutcome> {
    let (_, cmd) = extract_command_field(input.tool_input.as_ref())?;
    let cwd = input.cwd.as_deref().map_or_else(|| root.to_path_buf(), PathBuf::from);
    let tg = &config.token_guard;
    for inv in expand(cmd).invocations.iter().filter(|inv| dumps_whole_files(inv)) {
        for operand in inv.args.iter().chain(&inv.inputs).filter(|a| !a.starts_with('-')) {
            let abs = cwd.join(operand);
            if !abs.is_file() || !is_within_root(root, &abs) {
                continue;
            }
            let path_input = serde_json::json!({ "file_path": abs.to_string_lossy() });
            let rel = extract_rel_file(root, Some(&path_input))?;
            let limit =
                if is_noise_file(&rel) { tg.max_noise_read_lines } else { tg.max_text_lines };
            let Some(lines) = count_lines_if_heavy(&abs, limit) else {
                continue;
            };
            let bytes = fs::metadata(&abs).map_or(0, |m| m.len());
            record_block(root, estimate_tokens(usize::try_from(bytes).unwrap_or(usize::MAX)));
            return Some(HookOutcome::deny(format!(
                "Token Guard blocked `{}` of `{rel}` ({lines}+ lines > {limit}). Print a bounded range (`sed -n '1,200p'`, `head`, `Get-Content -TotalCount`), pipe it through a filter, or use `anvil grep` / `graph_read_symbol`.",
                inv.name
            )));
        }
    }
    None
}

fn dumps_whole_files(inv: &Invocation) -> bool {
    DUMPERS.contains(&inv.name.as_str())
        && !inv.captured_out
        && !inv.args.iter().any(|a| BOUNDING_FLAGS.contains(&a.to_ascii_lowercase().as_str()))
}
