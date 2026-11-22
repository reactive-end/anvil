//! AST symbol and edge extractor for CSS and SCSS stylesheets.

use anvil_core::traits::{EdgeKind, SymbolKind};
use tree_sitter::Node;

use super::ExtractState;

/// Visits a single CSS/SCSS AST node to extract `@import` edges, class selectors, and CSS vars.
pub fn visit_node(node: Node<'_>, state: &mut ExtractState<'_>) {
    match node.kind() {
        "import_statement" => extract_import(node, state),
        "class_selector" => extract_class_selector(node, state),
        "property_name" => extract_css_var(node, state),
        _ => {}
    }
}

fn extract_import(node: Node<'_>, state: &mut ExtractState<'_>) {
    let line = ExtractState::line_of(node);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "string_value" {
            let target = ExtractState::unquote(state.node_text(child)).to_owned();
            state.push_edge(None, target, EdgeKind::Imports, line);
            return;
        }
        if child.kind() == "call_expression" {
            let mut c_cursor = child.walk();
            for arg in child.named_children(&mut c_cursor) {
                if arg.kind() == "arguments"
                    && let Some(first) = arg.named_child(0)
                {
                    let target = ExtractState::unquote(state.node_text(first)).to_owned();
                    state.push_edge(None, target, EdgeKind::Imports, line);
                    return;
                }
            }
        }
    }
}

fn extract_class_selector(node: Node<'_>, state: &mut ExtractState<'_>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "class_name" {
            let name = state.node_text(child).trim();
            if !name.is_empty() {
                let full = format!(".{name}");
                state.push_symbol(
                    full.clone(),
                    full,
                    SymbolKind::Variable,
                    ExtractState::span_of(child),
                    Some("css_class".to_owned()),
                    true,
                );
            }
            return;
        }
    }
}

fn extract_css_var(node: Node<'_>, state: &mut ExtractState<'_>) {
    let name = state.node_text(node).trim();
    if name.starts_with("--") {
        state.push_symbol(
            name.to_owned(),
            name.to_owned(),
            SymbolKind::Constant,
            ExtractState::span_of(node),
            Some("css_var".to_owned()),
            true,
        );
    }
}
