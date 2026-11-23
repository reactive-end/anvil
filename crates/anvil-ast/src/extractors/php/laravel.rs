//! Laravel-specific AST extraction: routes (`Route::*`), Blade views, Inertia, Livewire, and DI bindings.

use anvil_core::traits::EdgeKind;
use tree_sitter::Node;

use crate::extractors::ExtractState;
use crate::extractors::http_route::{join_route_paths, normalize_route_target};

use super::{clean_php_name, enclosing_callable};

/// Handles `Route::*`, `Inertia::render`, `View::make`, and `Livewire::*` static calls.
pub fn visit_scoped_call(node: Node<'_>, state: &mut ExtractState<'_>) {
    let (Some(scope), Some(name_node)) = (
        node.child_by_field_name("scope"),
        node.child_by_field_name("name"),
    ) else {
        return;
    };
    let scope_short = clean_php_name(state.node_text(scope));
    let scope_tail = scope_short.rsplit('\\').next().unwrap_or(&scope_short);
    let method = state.node_text(name_node).trim();
    let args = node.child_by_field_name("arguments");

    match scope_tail {
        "Route" => handle_route_call(node, method, args, state),
        "Inertia" if method == "render" => emit_template_from_first_arg(node, args, "inertia", state),
        "View" if method == "make" => emit_template_from_first_arg(node, args, "blade", state),
        "Livewire" if matches!(method, "component" | "test") => {
            emit_template_from_first_arg(node, args, "livewire", state);
        }
        _ => {}
    }
}

/// Handles `view('...')` and `inertia('...')` global helper calls.
pub fn visit_function_call(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(func) = node.child_by_field_name("function") else {
        return;
    };
    let fname = clean_php_name(state.node_text(func));
    let args = node.child_by_field_name("arguments");
    match fname.as_str() {
        "view" => emit_template_from_first_arg(node, args, "blade", state),
        "inertia" => emit_template_from_first_arg(node, args, "inertia", state),
        _ => {}
    }
}

/// Handles `$this->app->bind(...)`/`singleton(...)`/`scoped(...)` and chained `Route::...->get(...)`.
pub fn visit_member_call(node: Node<'_>, state: &mut ExtractState<'_>) {
    let Some(name_node) = node.child_by_field_name("name") else {
        return;
    };
    let method = state.node_text(name_node).trim();
    let args = node.child_by_field_name("arguments");

    if matches!(method, "bind" | "singleton" | "scoped" | "bindIf" | "singletonIf") {
        extract_container_binding(node, args, state);
        return;
    }

    if is_laravel_route_method(method) && receiver_starts_with_route(node, state) {
        handle_route_call(node, method, args, state);
    }
}

fn is_laravel_route_method(method: &str) -> bool {
    matches!(
        method,
        "get"
            | "post"
            | "put"
            | "patch"
            | "delete"
            | "options"
            | "any"
            | "match"
            | "apiResource"
            | "resource"
            | "view"
            | "inertia"
    )
}

fn receiver_starts_with_route(node: Node<'_>, state: &ExtractState<'_>) -> bool {
    let mut cur = node.child_by_field_name("object");
    while let Some(obj) = cur {
        match obj.kind() {
            "scoped_call_expression" => {
                if let Some(scope) = obj.child_by_field_name("scope") {
                    let s = clean_php_name(state.node_text(scope));
                    return s.rsplit('\\').next() == Some("Route");
                }
                return false;
            }
            "member_call_expression" => cur = obj.child_by_field_name("object"),
            _ => return false,
        }
    }
    false
}

fn handle_route_call(
    node: Node<'_>,
    method: &str,
    args: Option<Node<'_>>,
    state: &mut ExtractState<'_>,
) {
    let Some(arg_list) = args else {
        return;
    };
    let positional = collect_argument_exprs(arg_list);
    if positional.is_empty() {
        return;
    }
    let line = ExtractState::line_of(node);
    let (prefix, group_controller) = collect_route_context(node, state);

    match method {
        "get" | "post" | "put" | "patch" | "delete" | "options" | "any" => {
            if let Some(subpath) = extract_php_string(positional[0], state) {
                emit_route_edge(method, &prefix, &subpath, line, state);
            }
            if let Some(handler) = positional.get(1) {
                emit_route_handler_call(*handler, group_controller.as_deref(), line, state);
            }
        }
        "match" => handle_match_route(&positional, &prefix, group_controller.as_deref(), line, state),
        "apiResource" | "resource" => {
            if let Some(resource_name) = extract_php_string(positional[0], state) {
                emit_resource_routes(&prefix, &resource_name, method == "resource", line, state);
            }
            if let Some(ctrl) = positional.get(1) {
                emit_route_handler_call(*ctrl, group_controller.as_deref(), line, state);
            }
        }
        "view" => handle_template_route(&positional, &prefix, "blade", line, state),
        "inertia" => handle_template_route(&positional, &prefix, "inertia", line, state),
        _ => {}
    }
}

fn handle_match_route(
    positional: &[Node<'_>],
    prefix: &str,
    group_controller: Option<&str>,
    line: u32,
    state: &mut ExtractState<'_>,
) {
    if positional.len() >= 2
        && let Some(subpath) = extract_php_string(positional[1], state)
    {
        for m in extract_string_array(positional[0], state) {
            emit_route_edge(&m, prefix, &subpath, line, state);
        }
    }
    if let Some(handler) = positional.get(2) {
        emit_route_handler_call(*handler, group_controller, line, state);
    }
}

fn handle_template_route(
    positional: &[Node<'_>],
    prefix: &str,
    scheme: &str,
    line: u32,
    state: &mut ExtractState<'_>,
) {
    if let Some(subpath) = extract_php_string(positional[0], state) {
        emit_route_edge("GET", prefix, &subpath, line, state);
    }
    if let Some(tpl_arg) = positional.get(1)
        && let Some(name) = extract_php_string(*tpl_arg, state)
    {
        state.push_edge(None, format!("{scheme}:{name}"), EdgeKind::RendersTemplate, line);
    }
}

fn emit_route_edge(
    method: &str,
    prefix: &str,
    subpath: &str,
    line: u32,
    state: &mut ExtractState<'_>,
) {
    let full = join_route_paths(prefix, subpath);
    if let Some(target) = normalize_route_target(method, &full) {
        state.push_edge(None, target, EdgeKind::ExposesRoute, line);
    }
}

fn emit_resource_routes(
    prefix: &str,
    resource: &str,
    include_web_forms: bool,
    line: u32,
    state: &mut ExtractState<'_>,
) {
    let base = join_route_paths(prefix, resource);
    let item = join_route_paths(&base, "{id}");
    for (m, p) in [
        ("GET", base.as_str()),
        ("POST", base.as_str()),
        ("GET", item.as_str()),
        ("PUT", item.as_str()),
        ("PATCH", item.as_str()),
        ("DELETE", item.as_str()),
    ] {
        if let Some(target) = normalize_route_target(m, p) {
            state.push_edge(None, target, EdgeKind::ExposesRoute, line);
        }
    }
    if include_web_forms {
        let create = join_route_paths(&base, "create");
        let edit = join_route_paths(&item, "edit");
        for p in [create, edit] {
            if let Some(target) = normalize_route_target("GET", &p) {
                state.push_edge(None, target, EdgeKind::ExposesRoute, line);
            }
        }
    }
}

fn emit_route_handler_call(
    handler: Node<'_>,
    group_controller: Option<&str>,
    line: u32,
    state: &mut ExtractState<'_>,
) {
    match handler.kind() {
        "array_creation_expression" => {
            let elems = collect_array_values(handler);
            if let Some(cls) = elems.first().and_then(|e| extract_class_const(*e, state)) {
                if let Some(act) = elems.get(1).and_then(|e| extract_php_string(*e, state)) {
                    state.push_edge(None, format!("{cls}::{act}"), EdgeKind::Calls, line);
                } else {
                    state.push_edge(None, cls, EdgeKind::Calls, line);
                }
            }
        }
        "class_constant_access_expression" => {
            if let Some(cls) = extract_class_const(handler, state) {
                state.push_edge(None, format!("{cls}::__invoke"), EdgeKind::Calls, line);
            }
        }
        "string" | "encapsed_string" => {
            if let Some(raw) = extract_php_string(handler, state) {
                if let Some((cls, act)) = raw.split_once('@') {
                    let cls_clean = clean_php_name(cls);
                    state.push_edge(None, format!("{cls_clean}::{act}"), EdgeKind::Calls, line);
                } else if let Some(ctrl) = group_controller {
                    state.push_edge(None, format!("{ctrl}::{raw}"), EdgeKind::Calls, line);
                }
            }
        }
        _ => {}
    }
}

fn collect_route_context(node: Node<'_>, state: &ExtractState<'_>) -> (String, Option<String>) {
    let mut prefixes: Vec<String> = Vec::new();
    let mut controller: Option<String> = None;

    if is_laravel_api_route_file(state.file) {
        prefixes.push("api".to_owned());
    }

    let mut group_prefixes: Vec<String> = Vec::new();
    let mut cur = node.parent();
    while let Some(parent) = cur {
        if parent.kind() == "member_call_expression"
            && let Some(m) = parent.child_by_field_name("name")
            && state.node_text(m).trim() == "group"
        {
            let (p, c) = extract_chain_prefix_and_controller(parent, state);
            if let Some(pref) = p {
                group_prefixes.push(pref);
            }
            if controller.is_none() {
                controller = c;
            }
        } else if parent.kind() == "scoped_call_expression"
            && let Some(m) = parent.child_by_field_name("name")
            && state.node_text(m).trim() == "group"
            && let Some(args) = parent.child_by_field_name("arguments")
            && let Some(first) = collect_argument_exprs(args).into_iter().next()
            && let Some(pref) = extract_array_key_string(first, "prefix", state)
        {
            group_prefixes.push(pref);
        }
        cur = parent.parent();
    }
    group_prefixes.reverse();
    prefixes.extend(group_prefixes);

    if node.kind() == "member_call_expression" {
        let (p, c) = extract_chain_prefix_and_controller(node, state);
        if let Some(pref) = p {
            prefixes.push(pref);
        }
        if controller.is_none() {
            controller = c;
        }
    }

    let mut combined = String::new();
    for seg in prefixes {
        combined = join_route_paths(&combined, &seg);
    }
    (combined, controller)
}

fn extract_chain_prefix_and_controller(
    call_node: Node<'_>,
    state: &ExtractState<'_>,
) -> (Option<String>, Option<String>) {
    let mut prefixes = Vec::new();
    let mut controller = None;
    let mut cur = call_node.child_by_field_name("object");
    while let Some(obj) = cur {
        match obj.kind() {
            "scoped_call_expression" | "member_call_expression" => {
                if let Some(name_node) = obj.child_by_field_name("name") {
                    let m = state.node_text(name_node).trim();
                    let args = obj.child_by_field_name("arguments");
                    if m == "prefix"
                        && let Some(a) = args.and_then(|al| collect_argument_exprs(al).into_iter().next())
                        && let Some(s) = extract_php_string(a, state)
                    {
                        prefixes.push(s);
                    } else if m == "controller"
                        && controller.is_none()
                        && let Some(a) = args.and_then(|al| collect_argument_exprs(al).into_iter().next())
                    {
                        controller = extract_class_const(a, state);
                    }
                }
                cur = obj.child_by_field_name("object");
            }
            _ => break,
        }
    }
    prefixes.reverse();
    let joined = if prefixes.is_empty() {
        None
    } else {
        let mut acc = String::new();
        for p in prefixes {
            acc = join_route_paths(&acc, &p);
        }
        Some(acc)
    };
    (joined, controller)
}

fn is_laravel_api_route_file(file: &str) -> bool {
    let lower = file.replace('\\', "/").to_ascii_lowercase();
    lower == "routes/api.php"
        || lower.ends_with("/routes/api.php")
        || lower.starts_with("routes/api/")
        || lower.contains("/routes/api/")
}

fn extract_container_binding(node: Node<'_>, args: Option<Node<'_>>, state: &mut ExtractState<'_>) {
    let Some(arg_list) = args else {
        return;
    };
    let exprs = collect_argument_exprs(arg_list);
    if exprs.len() < 2 {
        return;
    }
    let line = ExtractState::line_of(node);
    let source = enclosing_callable(node, state);
    if let Some(abstract_cls) = extract_class_const(exprs[0], state) {
        state.push_edge(source.clone(), abstract_cls, EdgeKind::Provides, line);
    }
    if let Some(concrete_cls) = extract_class_const(exprs[1], state) {
        state.push_edge(source, concrete_cls, EdgeKind::Provides, line);
    }
}

fn emit_template_from_first_arg(
    node: Node<'_>,
    args: Option<Node<'_>>,
    scheme: &str,
    state: &mut ExtractState<'_>,
) {
    let Some(arg_list) = args else {
        return;
    };
    let Some(first) = collect_argument_exprs(arg_list).into_iter().next() else {
        return;
    };
    let Some(name) = extract_php_string(first, state) else {
        return;
    };
    if !name.is_empty() {
        state.push_edge(
            enclosing_callable(node, state),
            format!("{scheme}:{name}"),
            EdgeKind::RendersTemplate,
            ExtractState::line_of(node),
        );
    }
}

fn collect_argument_exprs<'a>(arg_list: Node<'a>) -> Vec<Node<'a>> {
    let mut out = Vec::new();
    let mut cursor = arg_list.walk();
    for child in arg_list.named_children(&mut cursor) {
        if child.kind() == "argument" {
            let mut a_cursor = child.walk();
            // Skip optional named-argument label (`name: expr`) and pick the value expression.
            let expr = child
                .named_children(&mut a_cursor)
                .last();
            if let Some(e) = expr {
                out.push(e);
            }
        }
    }
    out
}

fn collect_array_values<'a>(arr: Node<'a>) -> Vec<Node<'a>> {
    let mut out = Vec::new();
    let mut cursor = arr.walk();
    for child in arr.named_children(&mut cursor) {
        if child.kind() == "array_element_initializer" {
            let mut e_cursor = child.walk();
            if let Some(val) = child.named_children(&mut e_cursor).last() {
                out.push(val);
            }
        }
    }
    out
}

fn extract_array_key_string(arr: Node<'_>, wanted_key: &str, state: &ExtractState<'_>) -> Option<String> {
    if arr.kind() != "array_creation_expression" {
        return None;
    }
    let mut cursor = arr.walk();
    for child in arr.named_children(&mut cursor) {
        if child.kind() != "array_element_initializer" {
            continue;
        }
        let mut parts = Vec::new();
        let mut e_cursor = child.walk();
        for p in child.named_children(&mut e_cursor) {
            parts.push(p);
        }
        if parts.len() == 2
            && extract_php_string(parts[0], state).as_deref() == Some(wanted_key)
        {
            return extract_php_string(parts[1], state);
        }
    }
    None
}

fn extract_string_array(arr: Node<'_>, state: &ExtractState<'_>) -> Vec<String> {
    if arr.kind() != "array_creation_expression" {
        return Vec::new();
    }
    collect_array_values(arr)
        .into_iter()
        .filter_map(|v| extract_php_string(v, state))
        .collect()
}

fn extract_class_const(node: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    if node.kind() != "class_constant_access_expression" {
        return None;
    }
    let raw = state.node_text(node).trim();
    let cls = raw.strip_suffix("::class")?;
    let cleaned = clean_php_name(cls);
    (!cleaned.is_empty()).then_some(cleaned)
}

fn extract_php_string(node: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    match node.kind() {
        "string" => {
            let unquoted = ExtractState::unquote(state.node_text(node)).trim();
            (!unquoted.is_empty()).then(|| unquoted.to_owned())
        }
        "encapsed_string" => {
            let unquoted = ExtractState::unquote(state.node_text(node)).trim();
            if unquoted.is_empty() {
                return None;
            }
            // Convert PHP interpolated route segments like `lookups/{$type}` into `lookups/{type}`.
            let normalized = unquoted.replace("{$", "{").replace('$', "");
            Some(normalized)
        }
        _ => None,
    }
}
