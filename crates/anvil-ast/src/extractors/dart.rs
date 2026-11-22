//! AST symbol and edge extractor for Dart and Flutter.

use anvil_core::traits::{EdgeKind, SymbolKind};
use tree_sitter::Node;

use super::ExtractState;

/// Visits a single Dart AST node to extract classes, widgets, functions, imports, and calls.
pub fn visit_node(node: Node<'_>, state: &mut ExtractState<'_>) {
    match node.kind() {
        "import_specification" => extract_uri_edge(node, EdgeKind::Imports, state),
        "library_export" => extract_uri_edge(node, EdgeKind::ReExports, state),
        "class_declaration" => extract_class(node, state),
        "mixin_declaration" | "enum_declaration" => extract_simple_type(node, state),
        "function_signature" => extract_function_sig(node, state),
        "call_expression" => extract_call(node, state),
        _ => {}
    }
}

fn enclosing_class(node: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    let mut cur = node.parent();
    while let Some(parent) = cur {
        if matches!(
            parent.kind(),
            "class_declaration" | "mixin_declaration" | "enum_declaration"
        ) {
            return parent
                .child_by_field_name("name")
                .map(|n| state.node_text(n).to_owned());
        }
        cur = parent.parent();
    }
    None
}

fn extract_uri_edge(node: Node<'_>, kind: EdgeKind, state: &mut ExtractState<'_>) {
    if let Some(uri_node) = node.child_by_field_name("uri") {
        let target = ExtractState::unquote(state.node_text(uri_node)).to_owned();
        state.push_edge(None, target, kind, ExtractState::line_of(node));
    }
}

fn extract_class(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let class_name = state.node_text(name_node).to_owned();
    let line = ExtractState::line_of(node);
    let exported = !class_name.starts_with('_');

    let mut is_widget = false;
    if let Some(super_node) = node.child_by_field_name("superclass")
        && let Some(type_node) = super_node.child_by_field_name("type")
    {
        let super_name = state.node_text(type_node).trim().to_owned();
        if matches!(
            super_name.as_str(),
            "StatelessWidget" | "StatefulWidget" | "HookWidget" | "ConsumerWidget"
        ) {
            is_widget = true;
        }
        state.push_edge(
            Some(class_name.clone()),
            super_name,
            EdgeKind::Extends,
            line,
        );
    }

    if let Some(ifaces) = node.child_by_field_name("interfaces") {
        let mut cursor = ifaces.walk();
        for child in ifaces.named_children(&mut cursor) {
            let iface = state.node_text(child).trim().to_owned();
            state.push_edge(
                Some(class_name.clone()),
                iface,
                EdgeKind::Implements,
                line,
            );
        }
    }

    let kind = if is_widget {
        SymbolKind::Component
    } else {
        SymbolKind::Class
    };
    state.push_symbol(
        class_name.clone(),
        class_name,
        kind,
        ExtractState::span_of(node),
        None,
        exported,
    );
}

fn extract_simple_type(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = state.node_text(name_node).to_owned();
    let exported = !name.starts_with('_');
    state.push_symbol(
        name.clone(),
        name,
        SymbolKind::Class,
        ExtractState::span_of(node),
        None,
        exported,
    );
}

fn extract_function_sig(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let name = state.node_text(name_node).to_owned();
    let exported = !name.starts_with('_');
    let span = dart_function_span(node);
    let (kind, qualified) = match enclosing_class(node, state) {
        Some(cls) => (SymbolKind::Method, format!("{cls}.{name}")),
        None => (SymbolKind::Function, name.clone()),
    };
    state.push_symbol(name, qualified, kind, span, None, exported);
}

fn dart_function_span(sig_node: Node<'_>) -> (u32, u32) {
    if let Some(parent) = sig_node.parent() {
        if parent.kind() == "function_declaration" {
            return ExtractState::span_of(parent);
        }
        if parent.kind() == "method_signature"
            && let Some(grand) = parent.parent()
            && matches!(grand.kind(), "method_declaration" | "declaration")
        {
            return ExtractState::span_of(grand);
        }
    }
    ExtractState::span_of(sig_node)
}

fn extract_call(node: Node<'_>, state: &mut ExtractState<'_>) {
    if let Some(fn_node) = node.child_by_field_name("function") {
        let callee = state.node_text(fn_node).trim().to_owned();
        state.push_edge(
            None,
            callee,
            EdgeKind::Calls,
            ExtractState::line_of(node),
        );
    }
}
