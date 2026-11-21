//! Tier 0 rule: block TODO/FIXME/HACK/XXX and long narrative/changelog comments.

use std::sync::OnceLock;

use anvil_core::traits::{Diagnostic, Severity};
use regex::Regex;
use tree_sitter::Node;

use super::RuleState;

static MARKER_RE: OnceLock<Option<Regex>> = OnceLock::new();
static CHANGELOG_RE: OnceLock<Option<Regex>> = OnceLock::new();

/// Inspects an AST comment node for forbidden markers and narrative/changelog bloat.
pub fn check_comment_node(node: Node<'_>, state: &mut RuleState<'_>) {
    let Ok(text) = node.utf8_text(state.source.as_bytes()) else {
        return;
    };

    check_markers(node, text, state);
    check_narrative(node, text, state);
}

fn check_markers(node: Node<'_>, text: &str, state: &mut RuleState<'_>) {
    let Some(re) = MARKER_RE
        .get_or_init(|| Regex::new(r"\b(TODO|FIXME|HACK|XXX)\b").ok())
        .as_ref()
    else {
        return;
    };

    for mat in re.find_iter(text) {
        let prefix = &text[..mat.start()];
        let rel_line = prefix.bytes().filter(|&b| b == b'\n').count() as u32;
        let line = node.start_position().row as u32 + 1 + rel_line;
        let col = if rel_line == 0 {
            node.start_position().column as u32 + 1 + prefix.len() as u32
        } else {
            1
        };
        state.diagnostics.push(Diagnostic {
            rule: "no-todo-comments".to_owned(),
            severity: Severity::Error,
            file: state.file.to_owned(),
            line,
            column: col,
            message: format!("Disallowed `{}` marker in comment", mat.as_str()),
            suggestion: Some(
                "Resolve the work item or track it in `.harness/tasks/` instead of comments"
                    .to_owned(),
            ),
        });
    }
}

fn check_narrative(node: Node<'_>, text: &str, state: &mut RuleState<'_>) {
    let trimmed = text.trim();
    if trimmed.contains("anvil-allow(") {
        return;
    }

    let is_doc_comment = trimmed.starts_with("/**") || trimmed.starts_with("///");
    let has_changelog = CHANGELOG_RE
        .get_or_init(|| {
            Regex::new(
                r"(?i)\b(changelog|modified\s+by|updated\s+by|changed\s+on|added\s+by|previously\s+this|history\s*:)\b",
            )
            .ok()
        })
        .as_ref()
        .is_some_and(|re| re.is_match(trimmed));

    let line_count = trimmed.lines().count();
    let is_long_narrative = !is_doc_comment && (line_count > 6 || trimmed.len() > 420);

    if has_changelog || is_long_narrative {
        let line = node.start_position().row as u32 + 1;
        let col = node.start_position().column as u32 + 1;
        state.diagnostics.push(Diagnostic {
            rule: "no-narrative-comments".to_owned(),
            severity: Severity::Warning,
            file: state.file.to_owned(),
            line,
            column: col,
            message: "Long narrative or changelog comment adds noise without contract value"
                .to_owned(),
            suggestion: Some(
                "Replace with concise purpose/invariant/JSDoc comment or move rationale to anvil-memory"
                    .to_owned(),
            ),
        });
    }
}
