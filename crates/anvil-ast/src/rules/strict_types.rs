//! Tier 0 rule: detect explicit `any` in TypeScript/TSX, `dynamic` in Dart,
//! and require `declare(strict_types=1);` in PHP files.

use anvil_core::traits::{Diagnostic, Language, Severity};
use tree_sitter::Node;

use super::{RuleState, has_anvil_allow};

/// Checks a single AST node for disallowed untyped escape hatches (`any`, `dynamic`).
pub fn check_node(node: Node<'_>, state: &mut RuleState<'_>) {
    match state.language {
        Language::TypeScript | Language::Tsx => check_ts_any(node, state),
        Language::Dart => check_dart_dynamic(node, state),
        _ => {}
    }
}

/// Enforces `declare(strict_types=1);` at the top of PHP files containing `<?php`.
pub fn check_php_strict_types(root: Node<'_>, state: &mut RuleState<'_>) {
    if state.language != Language::Php {
        return;
    }
    let mut php_tag_line: Option<u32> = None;
    let mut has_strict_declare = false;
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        match child.kind() {
            "php_tag" if php_tag_line.is_none() => {
                php_tag_line = Some(child.start_position().row as u32 + 1);
            }
            "declare_statement" if declare_enables_strict_types(child, state.source.as_bytes()) => {
                has_strict_declare = true;
                break;
            }
            _ => {}
        }
    }
    let Some(tag_line) = php_tag_line else {
        return;
    };
    if has_strict_declare {
        return;
    }
    if has_anvil_allow(state, tag_line, "require-strict-types")
        || has_anvil_allow(state, tag_line.saturating_add(1), "require-strict-types")
    {
        return;
    }
    state.diagnostics.push(Diagnostic {
        rule: "require-strict-types".to_owned(),
        severity: Severity::Error,
        file: state.file.to_owned(),
        line: tag_line,
        column: 1,
        message: "PHP file is missing mandatory `declare(strict_types=1);` header".to_owned(),
        suggestion: Some(
            "Add `declare(strict_types=1);` immediately after `<?php`".to_owned(),
        ),
    });
}

fn declare_enables_strict_types(node: Node<'_>, src: &[u8]) -> bool {
    let mut stack = vec![node];
    while let Some(cur) = stack.pop() {
        if cur.kind() == "declare_directive"
            && let Ok(text) = cur.utf8_text(src)
        {
            let compact: String = text
                .chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
                .to_ascii_lowercase();
            if compact == "strict_types=1" {
                return true;
            }
        }
        let mut cursor = cur.walk();
        for child in cur.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    false
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
    if has_anvil_allow(state, line, "no-explicit-any") {
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
    if has_anvil_allow(state, line, "no-unnecessary-dynamic") {
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
