//! Tier 0 rule: block silent permission assumptions (unhandled callbacks or unchecked permission APIs).

use anvil_core::traits::{Diagnostic, Language, Severity};
use tree_sitter::Node;

use super::{RuleState, has_anvil_allow};

const RULE_ID: &str = "no-unverified-permission";

/// Flags permission APIs and callbacks that ignore granted/denied outcomes.
pub fn check_node(node: Node<'_>, state: &mut RuleState<'_>) {
    match state.language {
        Language::Kotlin => check_kotlin_permission(node, state),
        Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Jsx => {
            check_js_ts_permission(node, state);
        }
        _ => {}
    }
}

fn check_kotlin_permission(node: Node<'_>, state: &mut RuleState<'_>) {
    match node.kind() {
        "call_expression" => check_kotlin_activity_result(node, state),
        "function_declaration" => check_kotlin_permissions_result(node, state),
        _ => {}
    }
}

fn check_kotlin_activity_result(node: Node<'_>, state: &mut RuleState<'_>) {
    let text = &state.source[node.byte_range()];
    if !text.contains("registerForActivityResult")
        || (!text.contains("RequestPermission") && !text.contains("RequestMultiplePermissions"))
    {
        return;
    }
    let Some(lambda) = find_callback_lambda(node) else {
        return;
    };
    let (param_name, body) = extract_lambda_param_and_body(lambda, state.source);
    let handled = !body.is_empty()
        && (contains_word(&body, "if")
            || contains_word(&body, "when")
            || has_unary_not(&body)
            || contains_word(&body, &param_name));
    if handled {
        return;
    }
    emit_diag(
        node,
        state,
        "`registerForActivityResult(RequestPermission)` callback ignores the granted/denied outcome — never assume runtime permissions are accepted automatically",
        "Branch on the `granted` result after window focus and provide a denial fallback (e.g. `Settings.ACTION_APP_NOTIFICATION_SETTINGS`)",
    );
}

fn check_kotlin_permissions_result(node: Node<'_>, state: &mut RuleState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    if &state.source[name_node.byte_range()] != "onRequestPermissionsResult" {
        return;
    }
    let Some(body_node) = node.child_by_field_name("body") else {
        return;
    };
    let code = strip_comments_and_strings(&state.source[body_node.byte_range()]);
    let checks_grant = contains_word(&code, "PERMISSION_GRANTED")
        || (contains_word(&code, "grantResults")
            && (contains_word(&code, "if")
                || contains_word(&code, "when")
                || contains_word(&code, "all")
                || contains_word(&code, "any")));
    if checks_grant {
        return;
    }
    emit_diag(
        node,
        state,
        "`onRequestPermissionsResult` ignores `grantResults` — verify permission state explicitly and handle denial",
        "Inspect `grantResults` against `PackageManager.PERMISSION_GRANTED` and provide a settings/UI fallback on denial",
    );
}

fn check_js_ts_permission(node: Node<'_>, state: &mut RuleState<'_>) {
    if node.kind() != "call_expression" {
        return;
    }
    let text = &state.source[node.byte_range()];
    if text.contains("Notification.requestPermission") {
        let bare_stmt = node.parent().is_some_and(|p| p.kind() == "expression_statement");
        let empty_then = has_empty_promise_then(node, state.source);
        if bare_stmt || empty_then {
            emit_diag(
                node,
                state,
                "`Notification.requestPermission()` result is ignored — never assume browser permission is granted",
                "Await the returned status (`status === 'granted'`) and handle `'denied'`/`'default'` states explicitly",
            );
        }
    } else if text.contains("navigator.permissions.query") && has_empty_promise_then(node, state.source) {
        emit_diag(
            node,
            state,
            "`navigator.permissions.query().then(...)` ignores the permission state callback result",
            "Inspect `status.state` (`'granted'`, `'prompt'`, `'denied'`) before acting on the permission",
        );
    }
}

fn has_empty_promise_then(inner_call: Node<'_>, source: &str) -> bool {
    let Some(member) = inner_call.parent().filter(|p| p.kind() == "member_expression") else {
        return false;
    };
    let Some(then_call) = member.parent().filter(|p| p.kind() == "call_expression") else {
        return false;
    };
    let then_text = &source[then_call.byte_range()];
    let Some((_, after_then)) = then_text.split_once(".then") else {
        return false;
    };
    let code = strip_comments_and_strings(after_then);
    code.contains("=> {}")
        || code.contains("=>{}")
        || code.contains("function(){}")
        || code.contains("function () {}")
}

fn emit_diag(node: Node<'_>, state: &mut RuleState<'_>, message: &str, suggestion: &str) {
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
        message: message.to_owned(),
        suggestion: Some(suggestion.to_owned()),
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
                        if matches!(arg_child.kind(), "lambda_literal" | "annotated_lambda") {
                            return Some(arg_child);
                        }
                    }
                }
            }
        }
    }
    None
}

fn extract_lambda_param_and_body(lambda_node: Node<'_>, source: &str) -> (String, String) {
    let raw = &source[lambda_node.byte_range()];
    let inner = raw
        .trim()
        .strip_prefix('{')
        .unwrap_or(raw)
        .strip_suffix('}')
        .unwrap_or(raw);
    let (param, body) = match inner.split_once("->") {
        Some((lhs, rhs)) => {
            let p = lhs
                .split_whitespace()
                .last()
                .filter(|s| s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or("it")
                .to_owned();
            (p, rhs)
        }
        None => ("it".to_owned(), inner),
    };
    (param, strip_comments_and_strings(body).trim().to_owned())
}

fn contains_word(haystack: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    haystack.match_indices(word).any(|(idx, _)| {
        let prev_ok = haystack[..idx]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
        let next_ok = haystack[idx + word.len()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphanumeric() && c != '_');
        prev_ok && next_ok
    })
}

fn has_unary_not(code: &str) -> bool {
    let bytes = code.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b != b'!' {
            continue;
        }
        let next = bytes.get(i + 1).copied();
        let prev = i.checked_sub(1).and_then(|p| bytes.get(p).copied());
        let is_neq_or_bang_bang = matches!(next, Some(b'=' | b'!'));
        let preceded_by_expr = prev.is_some_and(|p| p.is_ascii_alphanumeric() || matches!(p, b')' | b']' | b'!'));
        if !is_neq_or_bang_bang && !preceded_by_expr {
            return true;
        }
    }
    false
}

fn strip_comments_and_strings(input: &str) -> String {
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
        } else if matches!(bytes[i], b'"' | b'\'') {
            let quote = bytes[i];
            out.push(' ');
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i += 2;
                    continue;
                }
                if bytes[i] == quote {
                    i += 1;
                    break;
                }
                i += 1;
            }
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}
