//! AST symbol and edge extractor for Kotlin (including `@Composable`).

use anvil_core::traits::{EdgeKind, SymbolKind};
use tree_sitter::Node;

use super::ExtractState;

/// Visits a single Kotlin AST node to extract symbols, `@Composable` components, imports, and calls.
pub fn visit_node(node: Node<'_>, state: &mut ExtractState<'_>) {
    match node.kind() {
        "import" => extract_import(node, state),
        "class_declaration" => extract_class(node, state),
        "object_declaration" => extract_object(node, state),
        "function_declaration" => extract_function(node, state),
        "type_alias" => extract_type_alias(node, state),
        "call_expression" => extract_call(node, state),
        _ => {}
    }
}

fn modifiers_text<'a>(node: Node<'_>, state: &ExtractState<'a>) -> &'a str {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "modifiers" {
            return state.node_text(child);
        }
    }
    ""
}

fn is_public(modifiers: &str) -> bool {
    !modifiers.contains("private") && !modifiers.contains("internal")
}

fn enclosing_class(node: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    let mut cur = node.parent();
    while let Some(parent) = cur {
        if matches!(parent.kind(), "class_declaration" | "object_declaration") {
            return parent
                .child_by_field_name("name")
                .map(|n| state.node_text(n).to_owned());
        }
        cur = parent.parent();
    }
    None
}

fn extract_import(node: Node<'_>, state: &mut ExtractState<'_>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "qualified_identifier" {
            let target = state.node_text(child).trim().to_owned();
            state.push_edge(None, target, EdgeKind::Imports, ExtractState::line_of(node));
            return;
        }
    }
}

fn extract_class(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = state.node_text(name_node).to_owned();
    let line = ExtractState::line_of(node);
    let mods = modifiers_text(node, state);

    let mut is_interface = false;
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if !child.is_named() && state.node_text(child) == "interface" {
            is_interface = true;
            break;
        }
    }

    let kind = if is_interface {
        SymbolKind::Interface
    } else {
        SymbolKind::Class
    };
    state.push_symbol(
        name.clone(),
        name.clone(),
        kind,
        line,
        None,
        is_public(mods),
    );
    extract_delegations(node, &name, line, state);
}

fn extract_delegations(node: Node<'_>, class_name: &str, line: u32, state: &mut ExtractState<'_>) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "delegation_specifiers" {
            continue;
        }
        let mut d_cursor = child.walk();
        for spec in child.named_children(&mut d_cursor) {
            if spec.kind() != "delegation_specifier" {
                continue;
            }
            let Some(inner) = spec.named_child(0) else {
                continue;
            };
            let is_ctor = inner.kind() == "constructor_invocation";
            let raw = state.node_text(inner);
            let target = raw
                .split('(')
                .next()
                .unwrap_or(raw)
                .trim()
                .to_owned();
            let edge_kind = if is_ctor {
                EdgeKind::Extends
            } else {
                EdgeKind::Implements
            };
            state.push_edge(Some(class_name.to_owned()), target, edge_kind, line);
        }
    }
}

fn extract_object(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = state.node_text(name_node).to_owned();
    let mods = modifiers_text(node, state);
    state.push_symbol(
        name.clone(),
        name,
        SymbolKind::Class,
        ExtractState::line_of(node),
        Some("object".to_owned()),
        is_public(mods),
    );
}

fn extract_function(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = state.node_text(name_node).to_owned();
    let line = ExtractState::line_of(node);
    let mods = modifiers_text(node, state);
    let is_composable = mods.contains("@Composable");
    let cls = enclosing_class(node, state);

    let (kind, signature) = if is_composable {
        (SymbolKind::Component, Some("@Composable".to_owned()))
    } else if cls.is_some() {
        (SymbolKind::Method, None)
    } else {
        (SymbolKind::Function, None)
    };

    let qualified = match cls {
        Some(c) => format!("{c}.{name}"),
        None => name.clone(),
    };
    state.push_symbol(name, qualified, kind, line, signature, is_public(mods));
}

fn extract_type_alias(node: Node<'_>, state: &mut ExtractState<'_>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "identifier" {
            let name = state.node_text(child).to_owned();
            state.push_symbol(
                name.clone(),
                name,
                SymbolKind::TypeAlias,
                ExtractState::line_of(node),
                None,
                true,
            );
            return;
        }
    }
}

fn extract_call(node: Node<'_>, state: &mut ExtractState<'_>) {
    if let Some(target_node) = node.named_child(0) {
        let callee = state.node_text(target_node).trim().to_owned();
        state.push_edge(
            None,
            callee,
            EdgeKind::Calls,
            ExtractState::line_of(node),
        );
    }
}
