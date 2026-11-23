//! AST symbol and edge extractor for PHP 8.x and Laravel applications.

pub mod laravel;

use anvil_core::traits::{EdgeKind, SymbolKind};
use tree_sitter::Node;

use super::ExtractState;

const PHP_BUILTIN_TYPES: &[&str] = &[
    "int", "float", "string", "bool", "array", "object", "callable", "iterable", "mixed",
    "void", "never", "null", "false", "true", "self", "static", "parent",
];

/// Visits a single PHP AST node to extract symbols, imports, DI, calls, and Laravel edges.
pub fn visit_node(node: Node<'_>, state: &mut ExtractState<'_>) {
    match node.kind() {
        "namespace_use_declaration" => extract_namespace_use(node, state),
        "class_declaration" => extract_type_decl(node, SymbolKind::Class, state),
        "interface_declaration" => extract_type_decl(node, SymbolKind::Interface, state),
        "trait_declaration" => extract_type_decl(node, SymbolKind::Interface, state),
        "enum_declaration" => extract_type_decl(node, SymbolKind::TypeAlias, state),
        "use_declaration" => extract_trait_use(node, state),
        "method_declaration" => extract_method(node, state),
        "function_definition" => extract_function(node, state),
        "const_declaration" => extract_const(node, state),
        "object_creation_expression" => extract_new_expr(node, state),
        "scoped_call_expression" => extract_scoped_call(node, state),
        "function_call_expression" => extract_function_call(node, state),
        "member_call_expression" => laravel::visit_member_call(node, state),
        _ => {}
    }
}

fn extract_namespace_use(node: Node<'_>, state: &mut ExtractState<'_>) {
    let line = ExtractState::line_of(node);
    let mut group_prefix: Option<String> = None;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "namespace_name" => {
                group_prefix = Some(clean_php_name(state.node_text(child)));
            }
            "namespace_use_clause" => {
                if let Some(target) = extract_use_clause_target(child, group_prefix.as_deref(), state) {
                    state.push_edge(None, target, EdgeKind::Imports, line);
                }
            }
            "namespace_use_group" => {
                let mut g_cursor = child.walk();
                for item in child.named_children(&mut g_cursor) {
                    if item.kind() == "namespace_use_clause"
                        && let Some(target) = extract_use_clause_target(item, group_prefix.as_deref(), state)
                    {
                        state.push_edge(None, target, EdgeKind::Imports, line);
                    }
                }
            }
            _ => {}
        }
    }
}

fn extract_use_clause_target(
    clause: Node<'_>,
    prefix: Option<&str>,
    state: &ExtractState<'_>,
) -> Option<String> {
    let mut cursor = clause.walk();
    let name_node = clause.named_children(&mut cursor).find(|c| {
        matches!(c.kind(), "qualified_name" | "namespace_name" | "name")
    })?;
    let raw = clean_php_name(state.node_text(name_node));
    if raw.is_empty() {
        return None;
    }
    match prefix {
        Some(p) if !p.is_empty() => Some(format!("{p}\\{raw}")),
        _ => Some(raw),
    }
}

fn extract_type_decl(node: Node<'_>, kind: SymbolKind, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = state.node_text(name_node).trim().to_owned();
    if name.is_empty() {
        return;
    }
    let span = ExtractState::span_of(node);
    let line = span.0;
    let sig = match node.kind() {
        "trait_declaration" => Some(format!("trait {name}")),
        "enum_declaration" => Some(format!("enum {name}")),
        "interface_declaration" => Some(format!("interface {name}")),
        _ => None,
    };
    state.push_symbol(name.clone(), name.clone(), kind, span, sig, true);

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        match child.kind() {
            "base_clause" => emit_clause_targets(child, &name, EdgeKind::Extends, line, state),
            "class_interface_clause" => {
                emit_clause_targets(child, &name, EdgeKind::Implements, line, state);
            }
            _ => {}
        }
    }
}

fn emit_clause_targets(
    clause: Node<'_>,
    owner: &str,
    edge_kind: EdgeKind,
    line: u32,
    state: &mut ExtractState<'_>,
) {
    let mut cursor = clause.walk();
    for child in clause.named_children(&mut cursor) {
        if matches!(child.kind(), "name" | "qualified_name") {
            let target = clean_php_name(state.node_text(child));
            if !target.is_empty() {
                state.push_edge(Some(owner.to_owned()), target, edge_kind, line);
            }
        }
    }
}

fn extract_trait_use(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(owner) = enclosing_type_name(node, state) else {
        return;
    };
    let line = ExtractState::line_of(node);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "name" | "qualified_name") {
            let target = clean_php_name(state.node_text(child));
            if !target.is_empty() {
                state.push_edge(Some(owner.clone()), target, EdgeKind::Implements, line);
            }
        }
    }
}

fn extract_method(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = state.node_text(name_node).trim().to_owned();
    if name.is_empty() {
        return;
    }
    let owner = enclosing_type_name(node, state);
    let qualified = match &owner {
        Some(cls) => format!("{cls}::{name}"),
        None => name.clone(),
    };
    let exported = is_public_visibility(node, state);
    let span = ExtractState::span_of(node);
    let sig = build_callable_signature(node, &name, state);
    state.push_symbol(name.clone(), qualified.clone(), SymbolKind::Method, span, sig, exported);

    if exported || name == "__construct" {
        extract_injected_params(node, &qualified, state);
    }
}

fn extract_function(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = state.node_text(name_node).trim().to_owned();
    if name.is_empty() {
        return;
    }
    let span = ExtractState::span_of(node);
    let sig = build_callable_signature(node, &name, state);
    state.push_symbol(name.clone(), name.clone(), SymbolKind::Function, span, sig, true);
    extract_injected_params(node, &name, state);
}

fn extract_const(node: Node<'_>, state: &mut ExtractState<'_>) {
    let owner = enclosing_type_name(node, state);
    let exported = is_public_visibility(node, state);
    let span = ExtractState::span_of(node);
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() != "const_element" {
            continue;
        }
        let mut c_cursor = child.walk();
        if let Some(name_node) = child.named_children(&mut c_cursor).find(|n| n.kind() == "name") {
            let name = state.node_text(name_node).trim().to_owned();
            if name.is_empty() {
                continue;
            }
            let qualified = match &owner {
                Some(cls) => format!("{cls}::{name}"),
                None => name.clone(),
            };
            state.push_symbol(name, qualified, SymbolKind::Constant, span, None, exported);
        }
    }
}

fn extract_injected_params(callable: Node<'_>, source_sym: &str, state: &mut ExtractState<'_>) {
    let Some(params) = callable.child_by_field_name("parameters") else {
        return;
    };
    let mut cursor = params.walk();
    for param in params.named_children(&mut cursor) {
        if !matches!(
            param.kind(),
            "simple_parameter" | "property_promotion_parameter" | "variadic_parameter"
        ) {
            continue;
        }
        let Some(type_node) = param.child_by_field_name("type") else {
            continue;
        };
        let line = ExtractState::line_of(param);
        for type_name in collect_named_types(type_node, state) {
            state.push_edge(
                Some(source_sym.to_owned()),
                type_name,
                EdgeKind::Injects,
                line,
            );
        }
    }
}

fn collect_named_types(node: Node<'_>, state: &ExtractState<'_>) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![node];
    while let Some(cur) = stack.pop() {
        match cur.kind() {
            "named_type" | "qualified_name" | "name" => {
                let raw = clean_php_name(state.node_text(cur));
                let lower = raw.to_ascii_lowercase();
                if !raw.is_empty() && !PHP_BUILTIN_TYPES.contains(&lower.as_str()) {
                    out.push(raw);
                }
            }
            _ => {
                let mut cursor = cur.walk();
                for child in cur.named_children(&mut cursor) {
                    stack.push(child);
                }
            }
        }
    }
    out
}

fn extract_new_expr(node: Node<'_>, state: &mut ExtractState<'_>) {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if matches!(child.kind(), "name" | "qualified_name") {
            let target = clean_php_name(state.node_text(child));
            if let Some(clean) = ExtractState::sanitize_call_target(target) {
                state.push_edge(
                    enclosing_callable(node, state),
                    clean,
                    EdgeKind::Calls,
                    ExtractState::line_of(node),
                );
            }
            return;
        }
    }
}

fn extract_scoped_call(node: Node<'_>, state: &mut ExtractState<'_>) {
    laravel::visit_scoped_call(node, state);

    let Some(scope) = node.child_by_field_name("scope") else {
        return;
    };
    let scope_txt = clean_php_name(state.node_text(scope));
    if scope_txt.is_empty() || matches!(scope_txt.as_str(), "self" | "static" | "parent") {
        return;
    }
    let method_txt = node
        .child_by_field_name("name")
        .map(|n| state.node_text(n).trim())
        .unwrap_or("");
    let target = if method_txt.is_empty() {
        scope_txt
    } else {
        format!("{scope_txt}::{method_txt}")
    };
    if let Some(clean) = ExtractState::sanitize_call_target(target) {
        state.push_edge(
            enclosing_callable(node, state),
            clean,
            EdgeKind::Calls,
            ExtractState::line_of(node),
        );
    }
}

fn extract_function_call(node: Node<'_>, state: &mut ExtractState<'_>) {
    laravel::visit_function_call(node, state);

    let Some(func) = node.child_by_field_name("function") else {
        return;
    };
    let name = clean_php_name(state.node_text(func));
    if let Some(clean) = ExtractState::sanitize_call_target(name) {
        state.push_edge(
            enclosing_callable(node, state),
            clean,
            EdgeKind::Calls,
            ExtractState::line_of(node),
        );
    }
}

fn is_public_visibility(node: Node<'_>, state: &ExtractState<'_>) -> bool {
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "visibility_modifier" {
            return state.node_text(child).trim() == "public";
        }
    }
    true
}

fn build_callable_signature(node: Node<'_>, name: &str, state: &ExtractState<'_>) -> Option<String> {
    let params = node
        .child_by_field_name("parameters")
        .map(|n| state.node_text(n).trim())
        .unwrap_or("()");
    let ret = node
        .child_by_field_name("return_type")
        .map(|n| format!(": {}", state.node_text(n).trim()))
        .unwrap_or_default();
    let compact = format!("function {name}{params}{ret}");
    let one_line: String = compact.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.len() <= 160 {
        Some(one_line)
    } else {
        Some(format!("{}...", &one_line[..157]))
    }
}

pub(crate) fn enclosing_type_name(node: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    let mut cur = node.parent();
    while let Some(parent) = cur {
        if matches!(
            parent.kind(),
            "class_declaration" | "interface_declaration" | "trait_declaration" | "enum_declaration"
        ) {
            return parent
                .child_by_field_name("name")
                .map(|n| state.node_text(n).trim().to_owned());
        }
        cur = parent.parent();
    }
    None
}

pub(crate) fn enclosing_callable(node: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    let mut cur = node.parent();
    while let Some(parent) = cur {
        match parent.kind() {
            "method_declaration" => {
                let m = parent
                    .child_by_field_name("name")
                    .map(|n| state.node_text(n).trim())?;
                return Some(match enclosing_type_name(parent, state) {
                    Some(cls) => format!("{cls}::{m}"),
                    None => m.to_owned(),
                });
            }
            "function_definition" => {
                return parent
                    .child_by_field_name("name")
                    .map(|n| state.node_text(n).trim().to_owned());
            }
            _ => cur = parent.parent(),
        }
    }
    None
}

pub(crate) fn clean_php_name(raw: &str) -> String {
    raw.trim().trim_start_matches('\\').to_owned()
}
