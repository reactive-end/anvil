//! Tier 0 rule: block silent permission assumptions (e.g. empty `RequestPermission` callbacks).

use anvil_core::traits::{Diagnostic, Language, Severity};
use tree_sitter::Node;

use super::{RuleState, has_anvil_allow};

const RULE_ID: &str = "no-unverified-permission";

/// Flags permission request registrations that ignore the granted/denied outcome.
pub fn check_node(node: Node<'_>, state: &mut RuleState<'_>) {
    if state.language != Language::Kotlin || node.kind() != "call_expression" {
        return;
    }

    let text = &state.source[node.byte_range()];
    if !text.contains("registerForActivityResult")
        || (!text.contains("RequestPermission") && !text.contains("RequestMultiplePermissions"))
    {
        return;
    }

    let Some(lambda) = find_callback_lambda(node) else {
        return;
    };
    let body_text = extract_lambda_body_code(lambda, state.source);
    let handles_outcome = !body_text.is_empty()
        && (body_text.contains("if")
            || body_text.contains("when")
            || body_text.contains('!'));

    if handles_outcome {
        return;
    }

    let line = node.start_position().row as u32 + 1;
    if has_anvil_allow(state, line, RULE_ID) {
        return;
    }

    state.diagnostics.push(Diagnostic {
        rule: RULE_ID.to_owned(),
        severity: Severity::Error,
        file: state.file.to_owned(),
        line,
        column: node.start_position().column as u32 + 1,
        message: "`registerForActivityResult(RequestPermission)` callback does not handle the granted/denied outcome — never assume runtime permissions are accepted automatically".to_owned(),
        suggestion: Some("Branch on the `granted` result after window focus and provide a denial fallback (e.g. `Settings.ACTION_APP_NOTIFICATION_SETTINGS`)".to_owned()),
    });
}

fn find_callback_lambda(call_node: Node<'_>) -> Option<Node<'_>> {
    let mut cursor = call_node.walk();
    for child in call_node.named_children(&mut cursor) {
        if child.kind() == "annotated_lambda" || child.kind() == "lambda_literal" {
            return Some(child);
        }
        if child.kind() == "call_suffix" || child.kind() == "value_arguments" {
            let mut inner = child.walk();
            for sub in child.named_children(&mut inner) {
                if sub.kind() == "annotated_lambda" || sub.kind() == "lambda_literal" {
                    return Some(sub);
                }
                if sub.kind() == "value_argument" {
                    let mut arg_cur = sub.walk();
                    for arg_child in sub.named_children(&mut arg_cur) {
                        if arg_child.kind() == "lambda_literal"
                            || arg_child.kind() == "annotated_lambda"
                        {
                            return Some(arg_child);
                        }
                    }
                }
            }
        }
    }
    None
}

fn extract_lambda_body_code(lambda_node: Node<'_>, source: &str) -> String {
    let raw = &source[lambda_node.byte_range()];
    let inner = raw
        .trim()
        .strip_prefix('{')
        .unwrap_or(raw)
        .strip_suffix('}')
        .unwrap_or(raw);
    let after_arrow = match inner.split_once("->") {
        Some((_, body)) => body,
        None => inner,
    };
    strip_comments(after_arrow).trim().to_owned()
}

fn strip_comments(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'/' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}
