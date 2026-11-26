//! Single-pass AST traversal coordinating symbol/edge extraction and Tier 0 rules.

pub mod angular_html;
pub mod blade;
pub mod css;
pub mod dart;
pub mod http_route;
pub mod kotlin;
pub mod php;
pub mod python;
pub mod rust;
pub mod typescript;

use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_core::traits::{EdgeKind, EdgeRef, FileAstFacts, Language, SymbolDef, SymbolKind};
use tree_sitter::{Node, Tree};

use crate::rules::{self, RuleState};

/// Mutable collector for symbols and edges during the single AST walk.
pub struct ExtractState<'a> {
    pub file: &'a str,
    pub source: &'a str,
    pub language: Language,
    pub symbols: Vec<SymbolDef>,
    pub edges: Vec<EdgeRef>,
}

impl<'a> ExtractState<'a> {
    /// Creates an extraction collector for a single file.
    pub fn new(file: &'a str, source: &'a str, language: Language) -> Self {
        Self { file, source, language, symbols: Vec::new(), edges: Vec::new() }
    }

    /// Returns the UTF-8 slice for `node`, or empty string on invalid UTF-8.
    pub fn node_text(&self, node: Node<'_>) -> &'a str {
        node.utf8_text(self.source.as_bytes()).unwrap_or("")
    }

    /// Returns the 1-based start line of `node`.
    pub fn line_of(node: Node<'_>) -> u32 {
        node.start_position().row as u32 + 1
    }

    /// Returns the 1-based inclusive end line of `node`.
    pub fn end_line_of(node: Node<'_>) -> u32 {
        let start = Self::line_of(node);
        let end = node.end_position().row as u32 + 1;
        end.max(start)
    }

    /// Returns `(start_line, end_line)` 1-based inclusive span of `node`.
    pub fn span_of(node: Node<'_>) -> (u32, u32) {
        (Self::line_of(node), Self::end_line_of(node))
    }

    /// Trims matching surrounding single/double/backtick quotes.
    pub fn unquote(raw: &str) -> &str {
        let trimmed = raw.trim();
        if trimmed.len() >= 2
            && ((trimmed.starts_with('"') && trimmed.ends_with('"'))
                || (trimmed.starts_with('\'') && trimmed.ends_with('\''))
                || (trimmed.starts_with('`') && trimmed.ends_with('`')))
        {
            return &trimmed[1..trimmed.len() - 1];
        }
        trimmed
    }

    /// Ensures a call or import target is single-line, whitespace-free, and <= 80 chars.
    pub fn sanitize_call_target(target: String) -> Option<String> {
        if target.is_empty()
            || target.len() > 80
            || target.contains('\n')
            || target.contains('\r')
            || target.contains(char::is_whitespace)
        {
            return None;
        }
        Some(target)
    }

    /// Records a symbol definition with `(line, end_line)` 1-based inclusive span.
    pub fn push_symbol(
        &mut self,
        name: String,
        qualified_name: String,
        kind: SymbolKind,
        span: (u32, u32),
        signature: Option<String>,
        exported: bool,
    ) {
        if name.is_empty() {
            return;
        }
        let (line, end_line) = span;
        self.symbols.push(SymbolDef {
            name,
            qualified_name,
            kind,
            file: self.file.to_owned(),
            line,
            end_line: end_line.max(line),
            signature,
            exported,
        });
    }

    /// Records a directed dependency or usage edge.
    pub fn push_edge(
        &mut self,
        source_symbol: Option<String>,
        target_path_or_symbol: String,
        kind: EdgeKind,
        line: u32,
    ) {
        if target_path_or_symbol.is_empty() {
            return;
        }
        self.edges.push(EdgeRef {
            source_file: self.file.to_owned(),
            source_symbol,
            target_path_or_symbol,
            kind,
            line,
        });
    }
}

/// Runs a single pre-order AST walk to populate symbols, edges, and Tier 0 diagnostics.
pub fn collect_file_facts(
    path: &Path,
    normalized_file: &str,
    source: &str,
    blake3_hash: String,
    language: Language,
    tree: Option<&Tree>,
    config: &AnvilConfig,
) -> FileAstFacts {
    let mut extract_state = ExtractState::new(normalized_file, source, language);
    let root_node = tree.map(|t| t.root_node());
    let mut rule_state = RuleState::new(normalized_file, path, source, language, config, root_node);

    if let Some(root) = root_node {
        walk_single_pass(root, &mut extract_state, &mut rule_state);
        rules::check_file_level(Some(root), &mut rule_state);
    } else {
        rules::check_file_level(None, &mut rule_state);
    }

    FileAstFacts {
        path: normalized_file.to_owned(),
        blake3_hash,
        language,
        symbols: extract_state.symbols,
        edges: extract_state.edges,
        diagnostics: rule_state.diagnostics,
    }
}

fn walk_single_pass(
    root: Node<'_>,
    extract_state: &mut ExtractState<'_>,
    rule_state: &mut RuleState<'_>,
) {
    let mut cursor = root.walk();
    loop {
        let node = cursor.node();
        rules::check_node(node, rule_state);
        dispatch_extractor(node, extract_state);

        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return;
            }
        }
    }
}

fn dispatch_extractor(node: Node<'_>, state: &mut ExtractState<'_>) {
    match state.language {
        Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Jsx => {
            typescript::visit_node(node, state);
        }
        Language::Html => angular_html::visit_node(node, state),
        Language::Blade => blade::visit_node(node, state),
        Language::Css | Language::Scss => css::visit_node(node, state),
        Language::Python => python::visit_node(node, state),
        Language::Kotlin => kotlin::visit_node(node, state),
        Language::Dart => dart::visit_node(node, state),
        Language::Rust => rust::visit_node(node, state),
        Language::Php => php::visit_node(node, state),
        Language::GdScript
        | Language::GodotScene
        | Language::GdShader
        | Language::Json
        | Language::Yaml
        | Language::Markdown
        | Language::Other => {}
    }
}
