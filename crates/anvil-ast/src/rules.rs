//! Tier 0 deterministic AST rule orchestration and structured `anvil-allow` validation.

pub mod barrels;
pub mod comments;
pub mod escape_hatches;
pub mod root_hygiene;
pub mod strict_types;

use std::path::Path;
use std::sync::OnceLock;

use anvil_core::config::AnvilConfig;
use anvil_core::traits::{Diagnostic, Language};
use regex::Regex;
use tree_sitter::Node;

static ANVIL_ALLOW_RE: OnceLock<Option<Regex>> = OnceLock::new();

/// Per-file state shared across Tier 0 AST rules during a single walk.
pub struct RuleState<'a> {
    pub file: &'a str,
    pub path: &'a Path,
    pub source: &'a str,
    pub lines: Vec<&'a str>,
    pub language: Language,
    pub config: &'a AnvilConfig,
    pub diagnostics: Vec<Diagnostic>,
}

impl<'a> RuleState<'a> {
    /// Creates rule state for a single analyzed file.
    pub fn new(
        file: &'a str,
        path: &'a Path,
        source: &'a str,
        language: Language,
        config: &'a AnvilConfig,
    ) -> Self {
        Self {
            file,
            path,
            source,
            lines: source.lines().collect(),
            language,
            config,
            diagnostics: Vec::new(),
        }
    }

    /// Returns a 1-based line slice if in bounds.
    pub fn line_text(&self, line_1based: u32) -> Option<&'a str> {
        let idx = line_1based.checked_sub(1)? as usize;
        self.lines.get(idx).copied()
    }
}

/// Returns true when `kind` represents an AST comment node across supported grammars.
pub fn is_comment_kind(kind: &str) -> bool {
    matches!(
        kind,
        "comment"
            | "line_comment"
            | "block_comment"
            | "documentation_comment"
            | "documentation_block_comment"
            | "multiline_comment"
            | "html_comment"
            | "js_comment"
    )
}

/// Returns true if `line_1based` or the immediately preceding line has a structured
/// `anvil-allow(<rule>): <reason> [<ref>]` marker and `anvil-allow` is permitted by config.
pub fn has_structured_anvil_allow(state: &RuleState<'_>, line_1based: u32) -> bool {
    if !state
        .config
        .allowed_escapes
        .iter()
        .any(|item| item == "anvil-allow")
    {
        return false;
    }

    let Some(re) = ANVIL_ALLOW_RE
        .get_or_init(|| {
            Regex::new(
                r"anvil-allow\(\s*[A-Za-z0-9_:-]+\s*\)\s*:\s*[^\[\]\r\n]*[A-Za-z0-9][^\[\]\r\n]*\[[^\[\]\s][^\[\]\r\n]*\]",
            )
            .ok()
        })
        .as_ref()
    else {
        return false;
    };

    if state.line_text(line_1based).is_some_and(|l| re.is_match(l)) {
        return true;
    }

    if let Some(prev) = line_1based.checked_sub(1).and_then(|n| state.line_text(n))
        && re.is_match(prev)
    {
        return true;
    }

    false
}

/// Evaluates node-level Tier 0 AST rules during the single tree-sitter traversal.
pub fn check_node(node: Node<'_>, state: &mut RuleState<'_>) {
    if is_comment_kind(node.kind()) {
        comments::check_comment_node(node, state);
        escape_hatches::check_comment_escape(node, state);
        return;
    }

    escape_hatches::check_ast_escape(node, state);
    strict_types::check_node(node, state);
}

/// Evaluates file-level structural rules (barrels, root hygiene) after or around the AST walk.
pub fn check_file_level(root: Option<Node<'_>>, state: &mut RuleState<'_>) {
    if let Some(root_node) = root {
        barrels::check_barrel(root_node, state);
    }
    if let Some(diag) = root_hygiene::check_root_hygiene(state.path, state.file, state.config) {
        state.diagnostics.push(diag);
    }
}
