//! AST symbol and edge extractor for TypeScript, TSX, JavaScript, and JSX.

mod calls;
mod frameworks;
mod http_calls;
mod nest_routes;

use anvil_core::traits::{EdgeKind, Language, SymbolKind};
use tree_sitter::Node;

use super::ExtractState;

/// Visits a single AST node for TS/TSX/JS/JSX symbol and edge extraction.
pub fn visit_node(node: Node<'_>, state: &mut ExtractState<'_>) {
    match node.kind() {
        "import_statement" => extract_import(node, state),
        "export_statement" => extract_reexport(node, state),
        "function_declaration" | "generator_function_declaration" => extract_function(node, state),
        "interface_declaration" => extract_interface(node, state),
        "type_alias_declaration" => extract_type_alias(node, state),
        "class_declaration" => extract_class(node, state),
        "method_definition" => extract_method(node, state),
        "variable_declarator" => extract_variable(node, state),
        "call_expression" => extract_call(node, state),
        "jsx_opening_element" | "jsx_self_closing_element" => extract_jsx(node, state),
        _ => {}
    }
}

fn is_exported(node: Node<'_>) -> bool {
    node.parent().is_some_and(|p| p.kind() == "export_statement")
}

fn enclosing_class_node(node: Node<'_>) -> Option<Node<'_>> {
    let mut cur = node.parent();
    while let Some(parent) = cur {
        if parent.kind() == "class_declaration" {
            return Some(parent);
        }
        cur = parent.parent();
    }
    None
}

fn extract_import(node: Node<'_>, state: &mut ExtractState<'_>) {
    if let Some(source_node) = node.child_by_field_name("source") {
        let target = ExtractState::unquote(state.node_text(source_node)).to_owned();
        state.push_edge(None, target, EdgeKind::Imports, ExtractState::line_of(node));
    }
}

fn extract_reexport(node: Node<'_>, state: &mut ExtractState<'_>) {
    if let Some(source_node) = node.child_by_field_name("source") {
        let target = ExtractState::unquote(state.node_text(source_node)).to_owned();
        state.push_edge(None, target, EdgeKind::ReExports, ExtractState::line_of(node));
    }
}

fn extract_function(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = state.node_text(name_node).to_owned();
    let is_tsx = matches!(state.language, Language::Tsx | Language::Jsx);
    let kind = if is_tsx && name.starts_with(char::is_uppercase) {
        SymbolKind::Component
    } else {
        SymbolKind::Function
    };
    state.push_symbol(
        name.clone(),
        name,
        kind,
        ExtractState::span_of(node),
        None,
        is_exported(node),
    );
}

fn extract_interface(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = state.node_text(name_node).to_owned();
    let span = ExtractState::span_of(node);
    state.push_symbol(
        name.clone(),
        name.clone(),
        SymbolKind::Interface,
        span,
        None,
        is_exported(node),
    );

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "extends_type_clause" {
            let mut inner = child.walk();
            for item in child.named_children(&mut inner) {
                let target = state.node_text(item).trim().to_owned();
                state.push_edge(Some(name.clone()), target, EdgeKind::Extends, span.0);
            }
        }
    }
}

fn extract_type_alias(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = state.node_text(name_node).to_owned();
    state.push_symbol(
        name.clone(),
        name,
        SymbolKind::TypeAlias,
        ExtractState::span_of(node),
        None,
        is_exported(node),
    );
}

fn extract_class(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let class_name = state.node_text(name_node).to_owned();
    let span = ExtractState::span_of(node);
    let decorators = frameworks::collect_decorators(node);
    let (kind, signature) =
        frameworks::classify_class_decorators(&class_name, &decorators, span, state);
    state.push_symbol(
        class_name.clone(),
        class_name.clone(),
        kind,
        span,
        signature,
        is_exported(node),
    );
    frameworks::extract_class_heritage(node, &class_name, span.0, state);
}

fn extract_method(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let method_name = state.node_text(name_node).to_owned();
    let class_node = enclosing_class_node(node);
    let class_name = class_node
        .and_then(|c| c.child_by_field_name("name"))
        .map(|n| state.node_text(n).to_owned());
    let span = ExtractState::span_of(node);

    if method_name == "constructor" {
        frameworks::extract_di_params(node, class_name.as_deref(), span.0, state);
        return;
    }

    let qualified = match class_name {
        Some(cls) => format!("{cls}.{method_name}"),
        None => method_name.clone(),
    };
    nest_routes::extract_method_routes(node, class_node, &qualified, state);
    state.push_symbol(method_name, qualified, SymbolKind::Method, span, None, false);
}

fn extract_variable(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(decl) = node.parent() else {
        return;
    };
    if !matches!(decl.kind(), "lexical_declaration" | "variable_declaration") {
        return;
    }
    let Some(parent_stmt) = decl.parent() else {
        return;
    };
    if !matches!(parent_stmt.kind(), "program" | "export_statement") {
        return;
    }
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    if name_node.kind() != "identifier" {
        return;
    }
    let name = state.node_text(name_node).to_owned();
    let value_kind = node.child_by_field_name("value").map(|v| v.kind());
    let is_fn = matches!(value_kind, Some("arrow_function" | "function_expression"));
    let is_tsx = matches!(state.language, Language::Tsx | Language::Jsx);
    let kind = if is_fn && is_tsx && name.starts_with(char::is_uppercase) {
        SymbolKind::Component
    } else if is_fn {
        SymbolKind::Function
    } else if state.node_text(decl).starts_with("const ") {
        SymbolKind::Constant
    } else {
        SymbolKind::Variable
    };
    state.push_symbol(
        name.clone(),
        name,
        kind,
        ExtractState::span_of(node),
        None,
        parent_stmt.kind() == "export_statement",
    );
}

fn extract_call(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(fn_node) = node.child_by_field_name("function") else {
        return;
    };
    let line = ExtractState::line_of(node);
    if fn_node.kind() == "identifier"
        && state.node_text(fn_node).trim() == "require"
        && let Some(args) = node.child_by_field_name("arguments")
        && let Some(first) = args.named_child(0)
        && first.kind() == "string"
    {
        let target = ExtractState::unquote(state.node_text(first)).to_owned();
        state.push_edge(None, target, EdgeKind::Imports, line);
        return;
    }
    if let Some(callee) = calls::clean_callee(fn_node, state) {
        http_calls::extract_http_call(node, &callee, state);
        state.push_edge(None, callee, EdgeKind::Calls, line);
    }
}

fn extract_jsx(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let tag = state.node_text(name_node).trim().to_owned();
    if !tag.starts_with(char::is_uppercase) {
        return;
    }
    let line = ExtractState::line_of(node);
    state.push_edge(None, tag.clone(), EdgeKind::Calls, line);

    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "jsx_attribute"
            && let Some(prop_node) = child.named_child(0)
        {
            let prop_name = state.node_text(prop_node).trim();
            if !prop_name.is_empty() {
                state.push_edge(
                    None,
                    format!("{tag}.{prop_name}"),
                    EdgeKind::UsesProp,
                    ExtractState::line_of(child),
                );
            }
        }
    }
}
