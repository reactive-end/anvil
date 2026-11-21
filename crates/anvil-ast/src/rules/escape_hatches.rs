//! Tier 0 rule: block rationalization escape hatches unless `anvil-allow` is provided.

use std::sync::OnceLock;

use anvil_core::traits::{Diagnostic, Language, Severity};
use regex::Regex;
use tree_sitter::Node;

use super::{RuleState, has_structured_anvil_allow};

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
                r"(?i)(@ts-ignore\b|@ts-expect-error\b|eslint-disable(?:-(?:next-)?line)?\b|#\s*noqa\b|//\s*ignore(?:_for_file)?\s*:)",
            )
            .ok()
        })
        .as_ref()
    else {
        return;
    };

    for mat in re.find_iter(text) {
        let prefix = &text[..mat.start()];
        let rel_line = prefix.bytes().filter(|&b| b == b'\n').count() as u32;
        let line = node.start_position().row as u32 + 1 + rel_line;
        if has_structured_anvil_allow(state, line) {
            continue;
        }
        let col = if rel_line == 0 {
            node.start_position().column as u32 + 1 + prefix.len() as u32
        } else {
            1
        };
        push_escape_diag(state, line, col, mat.as_str().trim());
    }
}

/// Checks non-comment AST nodes for language-specific escape hatches (`!important`, `@Suppress`).
pub fn check_ast_escape(node: Node<'_>, state: &mut RuleState<'_>) {
    let kind = node.kind();
    if matches!(state.language, Language::Css | Language::Scss) && kind == "important" {
        let line = node.start_position().row as u32 + 1;
        if !has_structured_anvil_allow(state, line) {
            let col = node.start_position().column as u32 + 1;
            push_escape_diag(state, line, col, "!important");
        }
        return;
    }

    if state.language == Language::Kotlin && matches!(kind, "annotation" | "file_annotation") {
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
            let line = node.start_position().row as u32 + 1;
            if !has_structured_anvil_allow(state, line) {
                let col = node.start_position().column as u32 + 1;
                push_escape_diag(state, line, col, "@Suppress");
            }
        }
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
