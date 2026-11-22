//! Tier 0 deterministic AST rule orchestration and structured `anvil-allow` validation.

pub mod assumptions;
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
use tree_sitter::{Node, Point};

static ANVIL_ALLOW_RE: OnceLock<Option<Regex>> = OnceLock::new();

const ESCAPE_RULES: &[&str] = &[
    "no-ts-ignore",
    "no-ts-expect-error",
    "no-eslint-disable",
    "no-biome-ignore",
    "no-type-ignore",
    "no-noqa",
    "no-dart-ignore",
    "no-important",
    "no-suppress",
    "no-rust-allow",
    "no-unsafe",
    "no-unverified-permission",
];

/// Per-file state shared across Tier 0 AST rules during a single walk.
pub struct RuleState<'a> {
    pub file: &'a str,
    pub path: &'a Path,
    pub source: &'a str,
    pub lines: Vec<&'a str>,
    pub language: Language,
    pub config: &'a AnvilConfig,
    pub root: Option<Node<'a>>,
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
        root: Option<Node<'a>>,
    ) -> Self {
        Self {
            file,
            path,
            source,
            lines: source.lines().collect(),
            language,
            config,
            root,
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

/// Returns true if `line_1based` or the immediately preceding line contains a structured
/// `anvil-allow(<rule>): <reason> [<ref>]` marker inside a real AST comment node
/// whose `<rule>` matches `rule_id` (or `all`).
pub fn has_anvil_allow(state: &RuleState<'_>, line_1based: u32, rule_id: &str) -> bool {
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
                r"anvil-allow\(\s*([A-Za-z0-9_:-]+)\s*\)\s*:\s*[^\[\]\r\n]*[A-Za-z0-9][^\[\]\r\n]*\[[^\[\]\s][^\[\]\r\n]*\]",
            )
            .ok()
        })
        .as_ref()
    else {
        return false;
    };

    let candidates = [Some(line_1based), line_1based.checked_sub(1)];
    for cand_line in candidates.into_iter().flatten() {
        let Some(line_str) = state.line_text(cand_line) else {
            continue;
        };
        for caps in re.captures_iter(line_str) {
            let Some(full) = caps.get(0) else {
                continue;
            };
            let Some(rule_cap) = caps.get(1) else {
                continue;
            };
            if rule_matches(rule_cap.as_str(), rule_id)
                && is_in_comment_node(state.root, cand_line, full.start())
            {
                return true;
            }
        }
    }

    false
}

fn rule_matches(allowed_raw: &str, expected_raw: &str) -> bool {
    let allowed = allowed_raw.trim().to_ascii_lowercase();
    let expected = expected_raw.trim().to_ascii_lowercase();
    if allowed == "all" || allowed == expected {
        return true;
    }
    if let Some(short) = expected.strip_prefix("no-")
        && allowed == short
    {
        return true;
    }
    if matches!(
        expected.as_str(),
        "no-todo-comments" | "no-narrative-comments"
    ) && allowed == "comments"
    {
        return true;
    }
    allowed == "no-escape-hatches" && ESCAPE_RULES.contains(&expected.as_str())
}

fn is_in_comment_node(root: Option<Node<'_>>, line_1based: u32, col_bytes: usize) -> bool {
    let Some(root_node) = root else {
        return false;
    };
    let row = line_1based.saturating_sub(1) as usize;
    let point = Point::new(row, col_bytes);
    let Some(mut cur) = root_node.descendant_for_point_range(point, point) else {
        return false;
    };
    loop {
        if is_comment_kind(cur.kind()) {
            return true;
        }
        match cur.parent() {
            Some(parent) => cur = parent,
            None => return false,
        }
    }
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
    assumptions::check_node(node, state);
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
