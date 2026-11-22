//! AST symbol and edge extractor for Rust (`tree-sitter-rust`).

use anvil_core::traits::{EdgeKind, SymbolKind};
use tree_sitter::Node;

use super::ExtractState;

/// Visits a single Rust AST node to extract symbols, imports, trait impls, and calls.
pub fn visit_node(node: Node<'_>, state: &mut ExtractState<'_>) {
    match node.kind() {
        "use_declaration" => extract_use(node, state),
        "mod_item" => extract_mod(node, state),
        "function_item" | "function_signature_item" => extract_function(node, state),
        "struct_item" => extract_named_item(node, SymbolKind::Class, None, state),
        "enum_item" => extract_named_item(node, SymbolKind::Class, Some("enum"), state),
        "trait_item" => extract_named_item(node, SymbolKind::Interface, None, state),
        "const_item" => extract_named_item(node, SymbolKind::Constant, None, state),
        "type_item" => extract_named_item(node, SymbolKind::TypeAlias, None, state),
        "impl_item" => extract_impl(node, state),
        "call_expression" => extract_call(node, state),
        _ => {}
    }
}

fn is_public(node: Node<'_>) -> bool {
    let mut cursor = node.walk();
    node.children(&mut cursor).any(|c| c.kind() == "visibility_modifier")
}

fn enclosing_owner(node: Node<'_>, state: &ExtractState<'_>) -> Option<(String, bool)> {
    let mut cur = node.parent();
    while let Some(parent) = cur {
        match parent.kind() {
            "impl_item" => {
                let ty = parent.child_by_field_name("type").and_then(|n| clean_type_name(n, state))?;
                return Some((ty, false));
            }
            "trait_item" => {
                let name = parent.child_by_field_name("name").map(|n| state.node_text(n).trim().to_owned())?;
                return Some((name, is_public(parent)));
            }
            "function_item" => return None,
            _ => cur = parent.parent(),
        }
    }
    None
}

fn extract_named_item(node: Node<'_>, kind: SymbolKind, sig: Option<&str>, state: &mut ExtractState<'_>) {
    if let Some(name_node) = node.child_by_field_name("name") {
        let name = state.node_text(name_node).trim().to_owned();
        state.push_symbol(name.clone(), name, kind, ExtractState::span_of(node), sig.map(str::to_owned), is_public(node));
    }
}

fn extract_mod(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else { return };
    let name = state.node_text(name_node).trim().to_owned();
    let span = ExtractState::span_of(node);
    state.push_symbol(name.clone(), name.clone(), SymbolKind::Module, span, None, is_public(node));
    if node.child_by_field_name("body").is_none() {
        state.push_edge(None, name, EdgeKind::Imports, span.0);
    }
}

fn extract_function(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else { return };
    let name = state.node_text(name_node).trim().to_owned();
    let owner = enclosing_owner(node, state);
    let exported = is_public(node) || owner.as_ref().is_some_and(|(_, pub_trait)| *pub_trait);
    let (kind, qualified) = match owner {
        Some((owner_name, _)) => (SymbolKind::Method, format!("{owner_name}::{name}")),
        None => (SymbolKind::Function, name.clone()),
    };
    state.push_symbol(name, qualified, kind, ExtractState::span_of(node), None, exported);
}

fn extract_impl(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(type_name) = node.child_by_field_name("type").and_then(|n| clean_type_name(n, state)) else {
        return;
    };
    let span = ExtractState::span_of(node);
    let line = span.0;
    let trait_name = node.child_by_field_name("trait").and_then(|n| clean_type_name(n, state));
    let label = match &trait_name {
        Some(tr) => format!("impl {tr} for {type_name}"),
        None => format!("impl {type_name}"),
    };
    state.push_symbol(label.clone(), label, SymbolKind::Unknown, span, Some("impl".to_owned()), false);
    if let Some(tr) = trait_name {
        state.push_edge(Some(type_name), tr, EdgeKind::Implements, line);
    }
}

fn clean_type_name(node: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    match node.kind() {
        "type_identifier" | "primitive_type" | "identifier" | "scoped_type_identifier" | "scoped_identifier" => {
            ExtractState::sanitize_call_target(state.node_text(node).trim().to_owned())
        }
        "generic_type" | "reference_type" => node
            .child_by_field_name("type")
            .and_then(|inner| clean_type_name(inner, state)),
        _ => None,
    }
}

fn extract_use(node: Node<'_>, state: &mut ExtractState<'_>) {
    let kind = if is_public(node) {
        EdgeKind::ReExports
    } else {
        EdgeKind::Imports
    };
    if let Some(arg) = node.child_by_field_name("argument") {
        collect_use_paths(arg, "", ExtractState::line_of(node), kind, state);
    }
}

fn collect_use_paths(
    node: Node<'_>,
    prefix: &str,
    line: u32,
    kind: EdgeKind,
    state: &mut ExtractState<'_>,
) {
    match node.kind() {
        "identifier" | "scoped_identifier" | "crate" | "self" | "super" => {
            let text = state.node_text(node).trim();
            let path = if text == "self" && !prefix.is_empty() { prefix.to_owned() } else { join_rust_path(prefix, text) };
            if let Some(clean) = ExtractState::sanitize_call_target(path) {
                state.push_edge(None, clean, kind, line);
            }
        }
        "use_as_clause" => {
            if let Some(p) = node.child_by_field_name("path") {
                collect_use_paths(p, prefix, line, kind, state);
            }
        }
        "scoped_use_list" => {
            let sub = node
                .child_by_field_name("path")
                .map(|p| join_rust_path(prefix, state.node_text(p).trim()))
                .unwrap_or_else(|| prefix.to_owned());
            if let Some(list) = node.child_by_field_name("list") {
                collect_use_paths(list, &sub, line, kind, state);
            }
        }
        "use_list" => {
            let mut cursor = node.walk();
            for item in node.named_children(&mut cursor) {
                collect_use_paths(item, prefix, line, kind, state);
            }
        }
        "use_wildcard" => {
            if let Some(child) = node.named_child(0) {
                collect_use_paths(child, prefix, line, kind, state);
            } else if let Some(clean) = ExtractState::sanitize_call_target(prefix.to_owned()) {
                state.push_edge(None, clean, kind, line);
            }
        }
        _ => {}
    }
}

fn join_rust_path(prefix: &str, segment: &str) -> String {
    match (prefix.is_empty(), segment.is_empty()) {
        (true, _) => segment.to_owned(),
        (false, true) => prefix.to_owned(),
        (false, false) => format!("{prefix}::{segment}"),
    }
}

fn extract_call(node: Node<'_>, state: &mut ExtractState<'_>) {
    if let Some(fn_node) = node.child_by_field_name("function")
        && let Some(callee) = clean_rust_callee(fn_node, state)
    {
        state.push_edge(None, callee, EdgeKind::Calls, ExtractState::line_of(node));
    }
}

fn clean_rust_callee(node: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    match node.kind() {
        "identifier" | "scoped_identifier" => ExtractState::sanitize_call_target(state.node_text(node).trim().to_owned()),
        "generic_function" => node.child_by_field_name("function").and_then(|n| clean_rust_callee(n, state)),
        "field_expression" => {
            let field_name = state.node_text(node.child_by_field_name("field")?).trim();
            if let Some(val) = node.child_by_field_name("value")
                && let Some(recv) = clean_rust_receiver(val, state)
            {
                let combined = format!("{recv}.{field_name}");
                if combined.len() <= 80 {
                    return ExtractState::sanitize_call_target(combined);
                }
            }
            ExtractState::sanitize_call_target(field_name.to_owned())
        }
        _ => None,
    }
}

fn clean_rust_receiver(node: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    match node.kind() {
        "identifier" | "self" => ExtractState::sanitize_call_target(state.node_text(node).trim().to_owned()),
        "field_expression" => {
            let recv = clean_rust_receiver(node.child_by_field_name("value")?, state)?;
            let field = state.node_text(node.child_by_field_name("field")?).trim();
            ExtractState::sanitize_call_target(format!("{recv}.{field}"))
        }
        _ => None,
    }
}
