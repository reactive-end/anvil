//! Client HTTP call (`fetch`, `axios`, Angular `http`/`httpClient`) route extraction.

use anvil_core::traits::EdgeKind;
use tree_sitter::Node;

use crate::extractors::ExtractState;
use crate::extractors::http_route::normalize_route_target;

/// Emits `EdgeKind::CallsRoute` when `call_node` invokes an HTTP client with a route URL.
pub fn extract_http_call(call_node: Node<'_>, callee: &str, state: &mut ExtractState<'_>) {
    let Some(args) = call_node.child_by_field_name("arguments") else {
        return;
    };
    let Some((method, url_node)) = resolve_http_call(callee, args, state) else {
        return;
    };
    let Some(raw_url) = extract_url_node(url_node, state) else {
        return;
    };
    if raw_url.starts_with("./") || raw_url.starts_with("../") || !raw_url.contains('/') {
        return;
    }
    if let Some(target) = normalize_route_target(&method, &raw_url) {
        let source_symbol = enclosing_caller(call_node, state);
        state.push_edge(
            source_symbol,
            target,
            EdgeKind::CallsRoute,
            ExtractState::line_of(call_node),
        );
    }
}

fn enclosing_caller(node: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    let mut cur = node.parent();
    while let Some(parent) = cur {
        match parent.kind() {
            "method_definition" => {
                let name = state.node_text(parent.child_by_field_name("name")?).trim();
                let mut cls_cur = parent.parent();
                while let Some(c) = cls_cur {
                    if c.kind() == "class_declaration" {
                        if let Some(cls) = c.child_by_field_name("name") {
                            return Some(format!("{}.{name}", state.node_text(cls).trim()));
                        }
                        break;
                    }
                    cls_cur = c.parent();
                }
                return Some(name.to_owned());
            }
            "function_declaration" | "generator_function_declaration" => {
                let name = state.node_text(parent.child_by_field_name("name")?).trim();
                return Some(name.to_owned());
            }
            "variable_declarator" => {
                if let Some(name_node) = parent.child_by_field_name("name")
                    && name_node.kind() == "identifier"
                {
                    return Some(state.node_text(name_node).trim().to_owned());
                }
            }
            _ => {}
        }
        cur = parent.parent();
    }
    None
}

fn resolve_http_call<'a>(
    callee: &str,
    args: Node<'a>,
    state: &ExtractState<'_>,
) -> Option<(String, Node<'a>)> {
    if matches!(callee, "fetch" | "window.fetch" | "globalThis.fetch") {
        let url_node = args.named_child(0)?;
        let method = args
            .named_child(1)
            .and_then(|opts| extract_object_method(opts, state))
            .unwrap_or_else(|| "ANY".to_owned());
        return Some((method, url_node));
    }

    if callee == "axios" {
        let first = args.named_child(0)?;
        if first.kind() == "object" {
            let url_node = find_object_value(first, "url", state)?;
            let method = extract_object_method(first, state).unwrap_or_else(|| "GET".to_owned());
            return Some((method, url_node));
        }
        let method = args
            .named_child(1)
            .and_then(|opts| extract_object_method(opts, state))
            .unwrap_or_else(|| "GET".to_owned());
        return Some((method, first));
    }

    let (receiver, verb) = callee.rsplit_once('.')?;
    let recv_last = receiver.rsplit('.').next().unwrap_or(receiver).to_ascii_lowercase();
    let is_http_client =
        matches!(recv_last.as_str(), "http" | "httpclient" | "httpservice" | "axios" | "apiclient")
            || recv_last.ends_with("httpclient")
            || recv_last.ends_with("axios");

    if !is_http_client
        || !matches!(verb, "get" | "post" | "put" | "patch" | "delete" | "head" | "options")
    {
        return None;
    }

    let url_node = args.named_child(0)?;
    Some((verb.to_ascii_uppercase(), url_node))
}

fn extract_object_method(obj: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    let val = find_object_value(obj, "method", state)?;
    if !matches!(val.kind(), "string" | "template_string") {
        return None;
    }
    let raw = state.node_text(val);
    if val.kind() == "template_string" && raw.contains("${") {
        return None;
    }
    let clean = ExtractState::unquote(raw).trim();
    (!clean.is_empty()).then(|| clean.to_ascii_uppercase())
}

fn find_object_value<'a>(
    obj: Node<'a>,
    target_key: &str,
    state: &ExtractState<'_>,
) -> Option<Node<'a>> {
    if obj.kind() != "object" {
        return None;
    }
    let mut cursor = obj.walk();
    for child in obj.named_children(&mut cursor) {
        if child.kind() != "pair" {
            continue;
        }
        let Some(key_node) = child.child_by_field_name("key") else {
            continue;
        };
        if ExtractState::unquote(state.node_text(key_node)) == target_key {
            return child.child_by_field_name("value");
        }
    }
    None
}

fn extract_url_node(node: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    let (text, has_literal) = collect_url_parts(node, state)?;
    has_literal.then_some(text)
}

fn collect_url_parts(node: Node<'_>, state: &ExtractState<'_>) -> Option<(String, bool)> {
    match node.kind() {
        "string" | "template_string" => {
            let unquoted = ExtractState::unquote(state.node_text(node)).to_owned();
            Some((unquoted, true))
        }
        "parenthesized_expression" => {
            node.named_child(0).and_then(|inner| collect_url_parts(inner, state))
        }
        "binary_expression" => {
            let op = node.child_by_field_name("operator")?;
            if state.node_text(op).trim() != "+" {
                return None;
            }
            let left = node.child_by_field_name("left")?;
            let right = node.child_by_field_name("right")?;
            let (l_str, l_lit) = collect_url_parts(left, state)?;
            let (r_str, r_lit) = collect_url_parts(right, state)?;
            Some((format!("{l_str}{r_str}"), l_lit || r_lit))
        }
        _ => {
            let raw = state.node_text(node).trim();
            if raw.is_empty() || raw.contains(['\n', '\r']) || raw.len() > 80 {
                return None;
            }
            Some((format!("${{{raw}}}"), false))
        }
    }
}
