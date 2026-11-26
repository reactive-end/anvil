//! Tier 0 Laravel/PHP hygiene rules: block debug helpers (`dd`/`dump`/`ray`) and `env()` outside `config/`.

use anvil_core::traits::{Diagnostic, Language, Severity};
use tree_sitter::Node;

use super::{RuleState, has_anvil_allow};

const DEBUG_HELPERS: &[&str] = &["dd", "dump", "ddd", "ray", "var_dump", "print_r"];

/// Evaluates PHP/Laravel node-level hygiene rules during the single AST walk.
pub fn check_node(node: Node<'_>, state: &mut RuleState<'_>) {
    if state.language != Language::Php || node.kind() != "function_call_expression" {
        return;
    }
    let Some(func) = node.child_by_field_name("function") else {
        return;
    };
    let raw_name =
        func.utf8_text(state.source.as_bytes()).unwrap_or("").trim().trim_start_matches('\\');
    let lower = raw_name.to_ascii_lowercase();
    let line = node.start_position().row as u32 + 1;
    let col = node.start_position().column as u32 + 1;

    if DEBUG_HELPERS.contains(&lower.as_str()) {
        if has_anvil_allow(state, line, "no-debug-helpers") {
            return;
        }
        state.diagnostics.push(Diagnostic {
            rule: "no-debug-helpers".to_owned(),
            severity: Severity::Error,
            file: state.file.to_owned(),
            line,
            column: col,
            message: format!("Disallowed debug helper `{raw_name}()` left in PHP code"),
            suggestion: Some(
                "Remove debug helper or use structured logging (`Log::debug(...)`)".to_owned(),
            ),
        });
        return;
    }

    if lower == "env" && !is_allowed_env_file(state.file) {
        if has_anvil_allow(state, line, "no-env-outside-config") {
            return;
        }
        state.diagnostics.push(Diagnostic {
            rule: "no-env-outside-config".to_owned(),
            severity: Severity::Error,
            file: state.file.to_owned(),
            line,
            column: col,
            message: "Calling `env()` outside `config/` returns `null` when Laravel config cache is enabled".to_owned(),
            suggestion: Some(
                "Move the `env()` lookup into `config/*.php` and read it via `config('...')`".to_owned(),
            ),
        });
    }
}

fn is_allowed_env_file(file: &str) -> bool {
    let norm = file.replace('\\', "/").to_ascii_lowercase();
    norm.starts_with("config/")
        || norm.contains("/config/")
        || norm.starts_with("tests/")
        || norm.contains("/tests/")
}
