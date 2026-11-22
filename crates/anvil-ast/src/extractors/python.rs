//! AST symbol and edge extractor for Python.

use anvil_core::traits::{EdgeKind, SymbolKind};
use tree_sitter::Node;

use super::ExtractState;
use super::http_route::normalize_route_target;

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
    let span = ExtractState::span_of(node);
    let (kind, qualified) = match enclosing_class(node, state) {
        Some(cls) => (SymbolKind::Method, format!("{cls}.{name}")),
        None => (SymbolKind::Function, name.clone()),
    };
    extract_route_decorators(node, &qualified, state);
    state.push_symbol(name, qualified, kind, span, None, exported);
}

fn extract_route_decorators(fn_node: Node<'_>, qualified_fn: &str, state: &mut ExtractState<'_>) {
    let Some(parent) = fn_node.parent() else { return };
    if parent.kind() != "decorated_definition" {
        return;
    }
    let mut cursor = parent.walk();
    for child in parent.named_children(&mut cursor) {
        if child.kind() != "decorator" {
            continue;
        }
        emit_python_decorator_route(child, qualified_fn, state);
    }
}

fn emit_python_decorator_route(dec: Node<'_>, qualified_fn: &str, state: &mut ExtractState<'_>) {
    let Some(call) = dec.named_child(0) else { return };
    if call.kind() != "call" {
        return;
    }
    let Some(fn_expr) = call.child_by_field_name("function") else { return };
    if fn_expr.kind() != "attribute" {
        return;
    }
    let Some(attr) = fn_expr.child_by_field_name("attribute") else { return };
    let attr_name = state.node_text(attr).trim();
    let Some(args) = call.child_by_field_name("arguments") else { return };
    let Some(path) = extract_python_route_path(args, state) else { return };
    if !path.starts_with('/') {
        return;
    }

    let methods: Vec<String> = match attr_name {
        "get" | "post" | "put" | "patch" | "delete" | "head" | "options" => {
            vec![attr_name.to_owned()]
        }
        "route" | "api_route" => {
            let list = extract_python_methods_kwarg(args, state);
            if !list.is_empty() {
                list
            } else if attr_name == "api_route" {
                vec!["ANY".to_owned()]
            } else {
                vec!["GET".to_owned()]
            }
        }
        _ => return,
    };

    let line = ExtractState::line_of(dec);
    for method in methods {
        if let Some(target) = normalize_route_target(&method, &path) {
            state.push_edge(
                Some(qualified_fn.to_owned()),
                target,
                EdgeKind::ExposesRoute,
                line,
            );
        }
    }
}

fn extract_python_route_path(args: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    let mut cursor = args.walk();
    for arg in args.named_children(&mut cursor) {
        if arg.kind() == "string" {
            return Some(unquote_python_string(state.node_text(arg)));
        }
        if arg.kind() == "keyword_argument"
            && let Some(k) = arg.child_by_field_name("name")
            && matches!(state.node_text(k).trim(), "path" | "rule")
            && let Some(v) = arg.child_by_field_name("value")
            && v.kind() == "string"
        {
            return Some(unquote_python_string(state.node_text(v)));
        }
    }
    None
}

fn extract_python_methods_kwarg(args: Node<'_>, state: &ExtractState<'_>) -> Vec<String> {
    let mut out = Vec::new();
    let mut cursor = args.walk();
    for arg in args.named_children(&mut cursor) {
        if arg.kind() != "keyword_argument" {
            continue;
        }
        let Some(k) = arg.child_by_field_name("name") else { continue };
        if state.node_text(k).trim() != "methods" {
            continue;
        }
        let Some(v) = arg.child_by_field_name("value") else { continue };
        if !matches!(v.kind(), "list" | "tuple" | "set") {
            continue;
        }
        let mut inner = v.walk();
        for item in v.named_children(&mut inner) {
            if item.kind() == "string" {
                let m = unquote_python_string(state.node_text(item));
                if !m.is_empty() {
                    out.push(m);
                }
            }
        }
    }
    out
}

fn unquote_python_string(raw: &str) -> String {
    let stripped = raw
        .trim()
        .trim_start_matches(['f', 'F', 'r', 'R', 'u', 'U', 'b', 'B']);
    ExtractState::unquote(stripped).trim().to_owned()
}

fn extract_class(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let class_name = state.node_text(name_node).to_owned();
    let exported = !class_name.starts_with('_');
    let span = ExtractState::span_of(node);
    let line = span.0;
    state.push_symbol(
        class_name.clone(),
        class_name.clone(),
        SymbolKind::Class,
        span,
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
