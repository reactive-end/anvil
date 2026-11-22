//! Angular `@Component`, NestJS `@Module`/`@Injectable`, and class heritage helpers.

use anvil_core::traits::{EdgeKind, SymbolKind};
use tree_sitter::Node;

use crate::extractors::ExtractState;

/// Collects decorator AST nodes on a `class_declaration` or its parent `export_statement`.
pub fn collect_decorators<'a>(node: Node<'a>) -> Vec<Node<'a>> {
    let mut result = Vec::new();
    collect_child_decorators(node, &mut result);
    if let Some(parent) = node.parent()
        && parent.kind() == "export_statement"
    {
        collect_child_decorators(parent, &mut result);
    }
    result
}

fn collect_child_decorators<'a>(parent: Node<'a>, out: &mut Vec<Node<'a>>) {
    let mut cursor = parent.walk();
    for child in parent.children(&mut cursor) {
        if child.kind() == "decorator" {
            out.push(child);
        }
    }
}

/// Inspects Angular/NestJS decorators on a class and emits framework symbols/edges.
pub fn classify_class_decorators(
    class_name: &str,
    decorators: &[Node<'_>],
    span: (u32, u32),
    state: &mut ExtractState<'_>,
) -> (SymbolKind, Option<String>) {
    let line = span.0;
    for &dec in decorators {
        let Some((name, arg_obj)) = parse_decorator(dec, state) else {
            continue;
        };
        match name {
            "Component" => return handle_angular_component(class_name, arg_obj, span, state),
            "Module" => return handle_nest_module(class_name, arg_obj, line, state),
            "Injectable" => return (SymbolKind::Class, Some("@Injectable".to_owned())),
            _ => {}
        }
    }
    (SymbolKind::Class, None)
}

fn parse_decorator<'a>(
    dec: Node<'a>,
    state: &ExtractState<'a>,
) -> Option<(&'a str, Option<Node<'a>>)> {
    let expr = dec.named_child(0)?;
    match expr.kind() {
        "identifier" => Some((state.node_text(expr).trim(), None)),
        "member_expression" => {
            let prop = expr.child_by_field_name("property")?;
            Some((state.node_text(prop).trim(), None))
        }
        "call_expression" => {
            let fn_node = expr.child_by_field_name("function")?;
            let name = match fn_node.kind() {
                "identifier" => state.node_text(fn_node).trim(),
                "member_expression" => {
                    let prop = fn_node.child_by_field_name("property")?;
                    state.node_text(prop).trim()
                }
                _ => return None,
            };
            let arg_obj = expr
                .child_by_field_name("arguments")
                .and_then(|args| {
                    let mut cursor = args.walk();
                    args.named_children(&mut cursor)
                        .find(|n| n.kind() == "object")
                });
            Some((name, arg_obj))
        }
        _ => None,
    }
}

fn handle_angular_component(
    class_name: &str,
    arg_obj: Option<Node<'_>>,
    span: (u32, u32),
    state: &mut ExtractState<'_>,
) -> (SymbolKind, Option<String>) {
    let line = span.0;
    if let Some(obj) = arg_obj {
        if let Some(selector) = find_object_string(obj, "selector", state) {
            state.push_symbol(
                selector.clone(),
                format!("{class_name}::{selector}"),
                SymbolKind::Component,
                span,
                Some("angular_selector".to_owned()),
                true,
            );
        }
        if let Some(tpl) = find_object_string(obj, "templateUrl", state) {
            state.push_edge(Some(class_name.to_owned()), tpl, EdgeKind::RendersTemplate, line);
        }
    }
    (SymbolKind::Component, Some("@Component".to_owned()))
}

fn handle_nest_module(
    class_name: &str,
    arg_obj: Option<Node<'_>>,
    line: u32,
    state: &mut ExtractState<'_>,
) -> (SymbolKind, Option<String>) {
    if let Some(obj) = arg_obj {
        emit_nest_array(obj, "imports", class_name, EdgeKind::Imports, line, state);
        emit_nest_array(obj, "providers", class_name, EdgeKind::Provides, line, state);
        emit_nest_array(obj, "controllers", class_name, EdgeKind::Provides, line, state);
    }
    (SymbolKind::Module, Some("@Module".to_owned()))
}

fn find_object_pair_value<'a>(
    obj: Node<'a>,
    target_key: &str,
    state: &ExtractState<'a>,
) -> Option<Node<'a>> {
    let mut cursor = obj.walk();
    for child in obj.named_children(&mut cursor) {
        if child.kind() != "pair" {
            continue;
        }
        let Some(key_node) = child.child_by_field_name("key") else {
            continue;
        };
        let key = ExtractState::unquote(state.node_text(key_node));
        if key == target_key {
            return child.child_by_field_name("value");
        }
    }
    None
}

pub(super) fn find_object_string(
    obj: Node<'_>,
    key: &str,
    state: &ExtractState<'_>,
) -> Option<String> {
    let val = find_object_pair_value(obj, key, state)?;
    if !matches!(val.kind(), "string" | "template_string") {
        return None;
    }
    let raw = state.node_text(val);
    if val.kind() == "template_string" && raw.contains("${") {
        return None;
    }
    let unquoted = ExtractState::unquote(raw).trim();
    (!unquoted.is_empty()).then(|| unquoted.to_owned())
}

fn emit_nest_array(
    obj: Node<'_>,
    key: &str,
    class_name: &str,
    kind: EdgeKind,
    line: u32,
    state: &mut ExtractState<'_>,
) {
    let Some(arr) = find_object_pair_value(obj, key, state) else {
        return;
    };
    if arr.kind() != "array" {
        return;
    }
    let mut cursor = arr.walk();
    for item in arr.named_children(&mut cursor) {
        let target = match item.kind() {
            "identifier" => Some(state.node_text(item).trim().to_owned()),
            "member_expression" => {
                let text = state.node_text(item).trim();
                (!text.contains('\n') && text.len() <= 80).then(|| text.to_owned())
            }
            _ => None,
        };
        if let Some(target_name) = target {
            state.push_edge(Some(class_name.to_owned()), target_name, kind, line);
        }
    }
}

/// Extracts `extends` and `implements` edges from a class's `class_heritage` child.
pub fn extract_class_heritage(
    node: Node<'_>,
    class_name: &str,
    line: u32,
    state: &mut ExtractState<'_>,
) {
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() != "class_heritage" {
            continue;
        }
        let mut h_cursor = child.walk();
        for clause in child.children(&mut h_cursor) {
            let kind = match clause.kind() {
                "extends_clause" => EdgeKind::Extends,
                "implements_clause" => EdgeKind::Implements,
                _ => continue,
            };
            let mut c_cursor = clause.walk();
            for target in clause.named_children(&mut c_cursor) {
                let text = state.node_text(target).trim().to_owned();
                state.push_edge(Some(class_name.to_owned()), text, kind, line);
            }
        }
    }
}

/// Extracts constructor-injected dependency edges (`EdgeKind::Injects`).
pub fn extract_di_params(
    ctor: Node<'_>,
    class_name: Option<&str>,
    line: u32,
    state: &mut ExtractState<'_>,
) {
    let Some(params) = ctor.child_by_field_name("parameters") else {
        return;
    };
    let mut cursor = params.walk();
    for param in params.named_children(&mut cursor) {
        if let Some(type_ann) = param.child_by_field_name("type") {
            let mut t_cursor = type_ann.walk();
            for child in type_ann.named_children(&mut t_cursor) {
                let text = state.node_text(child).trim();
                if text.starts_with(char::is_uppercase) {
                    state.push_edge(class_name.map(str::to_owned), text.to_owned(), EdgeKind::Injects, line);
                }
            }
        }
    }
}
