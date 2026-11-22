//! NestJS `@Controller` + `@Get`/`@Post`/`@Put`/`@Patch`/`@Delete` route extraction.

use anvil_core::traits::EdgeKind;
use tree_sitter::Node;

use crate::extractors::ExtractState;
use crate::extractors::http_route::{join_route_paths, normalize_route_target};

use super::frameworks::{collect_decorators, find_object_string};

/// Emits `EdgeKind::ExposesRoute` edges for NestJS HTTP method decorators on `method_node`.
pub fn extract_method_routes(
    method_node: Node<'_>,
    class_node: Option<Node<'_>>,
    qualified_method: &str,
    state: &mut ExtractState<'_>,
) {
    let prefix = class_node
        .and_then(|cls| extract_controller_prefix(cls, state))
        .unwrap_or_default();

    let mut cur = method_node.prev_named_sibling();
    while let Some(sib) = cur {
        if sib.kind() != "decorator" {
            break;
        }
        emit_decorator_routes(sib, &prefix, qualified_method, state);
        cur = sib.prev_named_sibling();
    }
}

fn emit_decorator_routes(
    dec: Node<'_>,
    prefix: &str,
    qualified_method: &str,
    state: &mut ExtractState<'_>,
) {
    let Some((name, args)) = parse_decorator_call(dec, state) else {
        return;
    };
    let Some(http_method) = map_nest_http_decorator(name) else {
        return;
    };
    let line = ExtractState::line_of(dec);
    let subpaths = extract_route_subpaths(args, state);

    if subpaths.is_empty() {
        let full = join_route_paths(prefix, "");
        if let Some(target) = normalize_route_target(http_method, &full) {
            state.push_edge(
                Some(qualified_method.to_owned()),
                target,
                EdgeKind::ExposesRoute,
                line,
            );
        }
        return;
    }

    for sub in subpaths {
        let full = join_route_paths(prefix, &sub);
        if let Some(target) = normalize_route_target(http_method, &full) {
            state.push_edge(
                Some(qualified_method.to_owned()),
                target,
                EdgeKind::ExposesRoute,
                line,
            );
        }
    }
}

fn extract_controller_prefix(class_node: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    for dec in collect_decorators(class_node) {
        let Some((name, args)) = parse_decorator_call(dec, state) else {
            continue;
        };
        if name != "Controller" {
            continue;
        }
        let Some(arg_list) = args else {
            return Some(String::new());
        };
        let mut cursor = arg_list.walk();
        for arg in arg_list.named_children(&mut cursor) {
            if let Some(s) = extract_static_string(arg, state) {
                return Some(s);
            }
            if arg.kind() == "object"
                && let Some(path) = find_object_string(arg, "path", state)
            {
                return Some(path);
            }
        }
        return Some(String::new());
    }
    None
}

fn parse_decorator_call<'a>(
    dec: Node<'a>,
    state: &ExtractState<'a>,
) -> Option<(&'a str, Option<Node<'a>>)> {
    let expr = dec.named_child(0)?;
    if expr.kind() != "call_expression" {
        return None;
    }
    let fn_node = expr.child_by_field_name("function")?;
    let name = match fn_node.kind() {
        "identifier" => state.node_text(fn_node).trim(),
        "member_expression" => state.node_text(fn_node.child_by_field_name("property")?).trim(),
        _ => return None,
    };
    Some((name, expr.child_by_field_name("arguments")))
}

fn extract_route_subpaths(args: Option<Node<'_>>, state: &ExtractState<'_>) -> Vec<String> {
    let Some(arg_list) = args else {
        return Vec::new();
    };
    let Some(first) = arg_list.named_child(0) else {
        return Vec::new();
    };
    if let Some(s) = extract_static_string(first, state) {
        return vec![s];
    }
    if first.kind() == "array" {
        let mut out = Vec::new();
        let mut cursor = first.walk();
        for item in first.named_children(&mut cursor) {
            if let Some(s) = extract_static_string(item, state) {
                out.push(s);
            }
        }
        return out;
    }
    Vec::new()
}

fn extract_static_string(node: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    if !matches!(node.kind(), "string" | "template_string") {
        return None;
    }
    let raw = state.node_text(node);
    if node.kind() == "template_string" && raw.contains("${") {
        return None;
    }
    Some(ExtractState::unquote(raw).trim().to_owned())
}

fn map_nest_http_decorator(name: &str) -> Option<&'static str> {
    match name {
        "Get" => Some("GET"),
        "Post" => Some("POST"),
        "Put" => Some("PUT"),
        "Patch" => Some("PATCH"),
        "Delete" => Some("DELETE"),
        "Head" => Some("HEAD"),
        "Options" => Some("OPTIONS"),
        "All" => Some("ANY"),
        _ => None,
    }
}
