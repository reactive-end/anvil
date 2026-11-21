//! Tier 0 rule: detect explicit `any` in TypeScript/TSX and `dynamic` in Dart.

use anvil_core::traits::{Diagnostic, Language, Severity};
use tree_sitter::Node;

use super::{RuleState, has_structured_anvil_allow};

/// Checks a single AST node for disallowed untyped escape hatches (`any`, `dynamic`).
pub fn check_node(node: Node<'_>, state: &mut RuleState<'_>) {
    match state.language {
        Language::TypeScript | Language::Tsx => check_ts_any(node, state),
        Language::Dart => check_dart_dynamic(node, state),
        _ => {}
    }
}

fn check_ts_any(node: Node<'_>, state: &mut RuleState<'_>) {
    if node.kind() != "predefined_type" {
        return;
    }
    let Ok(text) = node.utf8_text(state.source.as_bytes()) else {
        return;
    };
    if text != "any" {
        return;
    }
    let line = node.start_position().row as u32 + 1;
    if has_structured_anvil_allow(state, line) {
        return;
    }
    let col = node.start_position().column as u32 + 1;
    state.diagnostics.push(Diagnostic {
        rule: "no-explicit-any".to_owned(),
        severity: Severity::Error,
        file: state.file.to_owned(),
        line,
        column: col,
        message: "Explicit `any` type weakens TypeScript type safety".to_owned(),
        suggestion: Some(
            "Use `unknown` with type narrowing or a concrete interface/generic".to_owned(),
        ),
    });
}

fn check_dart_dynamic(node: Node<'_>, state: &mut RuleState<'_>) {
    if node.kind() != "dynamic" {
        return;
    }
    let Ok(text) = node.utf8_text(state.source.as_bytes()) else {
        return;
    };
    if text != "dynamic" {
        return;
    }
    let line = node.start_position().row as u32 + 1;
    if has_structured_anvil_allow(state, line) {
        return;
    }
    let col = node.start_position().column as u32 + 1;
    state.diagnostics.push(Diagnostic {
        rule: "no-unnecessary-dynamic".to_owned(),
        severity: Severity::Error,
        file: state.file.to_owned(),
        line,
        column: col,
        message: "Avoid explicit `dynamic` type in Dart; prefer `Object?` or concrete types"
            .to_owned(),
        suggestion: Some("Replace `dynamic` with `Object?` or a typed model".to_owned()),
    });
}
