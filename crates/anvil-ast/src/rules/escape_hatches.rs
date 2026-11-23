//! Tier 0 rule: block rationalization escape hatches unless `anvil-allow` is provided.

use std::sync::OnceLock;

use anvil_core::traits::{Diagnostic, Language, Severity};
use regex::Regex;
use tree_sitter::Node;

use super::{RuleState, has_anvil_allow};

static COMMENT_ESCAPE_RE: OnceLock<Option<Regex>> = OnceLock::new();
static KOTLIN_SUPPRESS_RE: OnceLock<Option<Regex>> = OnceLock::new();

/// Checks AST comment nodes for forbidden lint/typechecker suppression directives.
pub fn check_comment_escape(node: Node<'_>, state: &mut RuleState<'_>) {
    let Ok(text) = node.utf8_text(state.source.as_bytes()) else {
        return;
    };
    let Some(re) = COMMENT_ESCAPE_RE
        .get_or_init(|| {
            Regex::new(
                r"(?i)(@ts-ignore\b|@ts-expect-error\b|eslint-disable(?:-(?:next-)?line)?\b|\bbiome-ignore(?:-(?:all|start|end))?\b|#\s*type\s*:\s*ignore\b|#\s*noqa\b|//\s*ignore(?:_for_file)?\s*:|@phpstan-ignore(?:-next-line|-line)?\b|@psalm-suppress\b|\bphpcs:(?:ignore(?:File)?|disable)\b)",
            )
            .ok()
        })
        .as_ref()
    else {
        return;
    };

    for mat in re.find_iter(text) {
        let matched = mat.as_str().trim();
        let Some(allow_rule) = classify_comment_escape(matched, state.language) else {
            continue;
        };
        let prefix = &text[..mat.start()];
        let rel_line = prefix.bytes().filter(|&b| b == b'\n').count() as u32;
        let line = node.start_position().row as u32 + 1 + rel_line;
        if has_anvil_allow(state, line, allow_rule) {
            continue;
        }
        let col = if rel_line == 0 {
            node.start_position().column as u32 + 1 + prefix.len() as u32
        } else {
            1
        };
        push_escape_diag(state, line, col, matched);
    }
}

fn classify_comment_escape(matched: &str, lang: Language) -> Option<&'static str> {
    let lower = matched.to_ascii_lowercase();
    if lower.starts_with("@ts-ignore") {
        Some("no-ts-ignore")
    } else if lower.starts_with("@ts-expect-error") {
        Some("no-ts-expect-error")
    } else if lower.starts_with("eslint-disable") {
        Some("no-eslint-disable")
    } else if lower.starts_with("biome-ignore") {
        Some("no-biome-ignore")
    } else if lower.starts_with('#') {
        if lang != Language::Python {
            return None;
        }
        if lower.contains("noqa") {
            Some("no-noqa")
        } else {
            Some("no-type-ignore")
        }
    } else if lower.starts_with("//") {
        (lang == Language::Dart).then_some("no-dart-ignore")
    } else if lower.starts_with("@phpstan-ignore") {
        (lang == Language::Php).then_some("no-phpstan-ignore")
    } else if lower.starts_with("@psalm-suppress") {
        (lang == Language::Php).then_some("no-psalm-suppress")
    } else if lower.starts_with("phpcs:") {
        (lang == Language::Php).then_some("no-phpcs-ignore")
    } else {
        None
    }
}

/// Checks non-comment AST nodes for language-specific escape hatches (`!important`, `@Suppress`,
/// Rust `#[allow(...)]`/`unsafe`, and PHP `@` error suppression).
pub fn check_ast_escape(node: Node<'_>, state: &mut RuleState<'_>) {
    let kind = node.kind();
    match state.language {
        Language::Css | Language::Scss if kind == "important" => {
            report_if_disallowed(node, state, "no-important", "!important");
        }
        Language::Kotlin if matches!(kind, "annotation" | "file_annotation") => {
            check_kotlin_suppress(node, state);
        }
        Language::Rust => check_rust_escape(node, state),
        Language::Php if kind == "error_suppression_expression" => {
            report_if_disallowed(node, state, "no-php-error-suppression", "@");
        }
        _ => {}
    }
}

fn check_kotlin_suppress(node: Node<'_>, state: &mut RuleState<'_>) {
    let Ok(text) = node.utf8_text(state.source.as_bytes()) else {
        return;
    };
    let Some(re) = KOTLIN_SUPPRESS_RE
        .get_or_init(|| Regex::new(r"^@(?:file:)?Suppress(?:Warnings)?\b").ok())
        .as_ref()
    else {
        return;
    };
    if re.is_match(text.trim()) {
        report_if_disallowed(node, state, "no-suppress", "@Suppress");
    }
}

fn check_rust_escape(node: Node<'_>, state: &mut RuleState<'_>) {
    match node.kind() {
        "attribute_item" | "inner_attribute_item" => check_rust_attribute(node, state),
        "unsafe_block" => {
            report_if_disallowed(node, state, "no-unsafe", "unsafe");
        }
        "function_modifiers" | "impl_item" | "trait_item" => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if !child.is_named()
                    && child.utf8_text(state.source.as_bytes()).ok() == Some("unsafe")
                {
                    report_if_disallowed(child, state, "no-unsafe", "unsafe");
                    return;
                }
            }
        }
        _ => {}
    }
}

fn check_rust_attribute(node: Node<'_>, state: &mut RuleState<'_>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "attribute" {
            continue;
        }
        let Some(attr_name) = child.named_child(0) else {
            continue;
        };
        let Ok(name) = attr_name.utf8_text(state.source.as_bytes()) else {
            continue;
        };
        match name {
            "allow" => {
                report_if_disallowed(node, state, "no-rust-allow", "#[allow(...)]");
                return;
            }
            "expect" => {
                report_if_disallowed(node, state, "no-rust-allow", "#[expect(...)]");
                return;
            }
            "cfg_attr" => {
                if let Some(args) = child.child_by_field_name("arguments").or_else(|| child.named_child(1))
                    && cfg_attr_contains_allow_or_expect(args, state.source.as_bytes())
                {
                    report_if_disallowed(node, state, "no-rust-allow", "#[cfg_attr(..., allow(...))]");
                    return;
                }
            }
            _ => {}
        }
    }
}

fn cfg_attr_contains_allow_or_expect(node: Node<'_>, src: &[u8]) -> bool {
    let mut stack = vec![node];
    while let Some(cur) = stack.pop() {
        if matches!(
            cur.kind(),
            "string_literal" | "raw_string_literal" | "char_literal"
        ) {
            continue;
        }
        if cur.kind() == "identifier"
            && let Ok(txt) = cur.utf8_text(src)
            && matches!(txt, "allow" | "expect")
            && cur.next_sibling().is_some_and(|n| n.kind() == "token_tree")
        {
            return true;
        }
        let mut cursor = cur.walk();
        for child in cur.named_children(&mut cursor) {
            stack.push(child);
        }
    }
    false
}

fn report_if_disallowed(node: Node<'_>, state: &mut RuleState<'_>, rule_id: &str, hatch: &str) {
    let line = node.start_position().row as u32 + 1;
    if !has_anvil_allow(state, line, rule_id) {
        let col = node.start_position().column as u32 + 1;
        push_escape_diag(state, line, col, hatch);
    }
}

fn push_escape_diag(state: &mut RuleState<'_>, line: u32, column: u32, hatch: &str) {
    state.diagnostics.push(Diagnostic {
        rule: "no-escape-hatches".to_owned(),
        severity: Severity::Error,
        file: state.file.to_owned(),
        line,
        column,
        message: format!("Disallowed escape hatch `{hatch}` without structured anvil-allow"),
        suggestion: Some(
            "Fix the underlying issue or document with `anvil-allow(<rule>): <reason> [<ref>]`"
                .to_owned(),
        ),
    });
}
