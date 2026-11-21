//! AST symbol and edge extractor for Python.

use anvil_core::traits::{EdgeKind, SymbolKind};
use tree_sitter::Node;

use super::ExtractState;

/// Visits a single Python AST node to extract symbols, imports, inheritance, and calls.
pub fn visit_node(node: Node<'_>, state: &mut ExtractState<'_>) {
    match node.kind() {
        "import_statement" => extract_import(node, state),
        "import_from_statement" => extract_import_from(node, state),
        "function_definition" => extract_function(node, state),
        "class_definition" => extract_class(node, state),
        "call" => extract_call(node, state),
        _ => {}
    }
}

fn enclosing_class(node: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    let mut cur = node.parent();
    while let Some(parent) = cur {
        if parent.kind() == "class_definition" {
            return parent
                .child_by_field_name("name")
                .map(|n| state.node_text(n).to_owned());
        }
        cur = parent.parent();
    }
    None
}

fn extract_import(node: Node<'_>, state: &mut ExtractState<'_>) {
    let line = ExtractState::line_of(node);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        let target = match child.kind() {
            "dotted_name" => state.node_text(child).trim().to_owned(),
            "aliased_import" => child
                .child_by_field_name("name")
                .map(|n| state.node_text(n).trim().to_owned())
                .unwrap_or_default(),
            _ => continue,
        };
        state.push_edge(None, target, EdgeKind::Imports, line);
    }
}

fn extract_import_from(node: Node<'_>, state: &mut ExtractState<'_>) {
    let line = ExtractState::line_of(node);
    if let Some(mod_node) = node.child_by_field_name("module_name") {
        let target = state.node_text(mod_node).trim().to_owned();
        state.push_edge(None, target, EdgeKind::Imports, line);
    }
}

fn extract_function(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = state.node_text(name_node).to_owned();
    let exported = !name.starts_with('_');
    let line = ExtractState::line_of(node);
    let (kind, qualified) = match enclosing_class(node, state) {
        Some(cls) => (SymbolKind::Method, format!("{cls}.{name}")),
        None => (SymbolKind::Function, name.clone()),
    };
    state.push_symbol(name, qualified, kind, line, None, exported);
}

fn extract_class(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let class_name = state.node_text(name_node).to_owned();
    let exported = !class_name.starts_with('_');
    let line = ExtractState::line_of(node);
    state.push_symbol(
        class_name.clone(),
        class_name.clone(),
        SymbolKind::Class,
        line,
        None,
        exported,
    );

    if let Some(supers) = node.child_by_field_name("superclasses") {
        let mut cursor = supers.walk();
        for arg in supers.named_children(&mut cursor) {
            if matches!(arg.kind(), "identifier" | "attribute") {
                let target = state.node_text(arg).trim().to_owned();
                state.push_edge(Some(class_name.clone()), target, EdgeKind::Extends, line);
            }
        }
    }
}

fn extract_call(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(fn_node) = node.child_by_field_name("function") else {
        return;
    };
    let callee = state.node_text(fn_node).trim().to_owned();
    state.push_edge(
        None,
        callee,
        EdgeKind::Calls,
        ExtractState::line_of(node),
    );
}
