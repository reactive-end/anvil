//! Angular `@Component`, NestJS `@Module`/`@Injectable`, and class heritage helpers.

use anvil_core::traits::{EdgeKind, SymbolKind};
use tree_sitter::Node;

use crate::extractors::ExtractState;

/// Collects decorator strings on a `class_declaration` or its parent `export_statement`.
pub fn collect_decorators<'a>(node: Node<'_>, state: &ExtractState<'a>) -> Vec<&'a str> {
    let mut result = Vec::new();
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if child.kind() == "decorator" {
            result.push(state.node_text(child));
        }
    }
    if let Some(parent) = node.parent()
        && parent.kind() == "export_statement"
    {
        let mut p_cursor = parent.walk();
        for child in parent.children(&mut p_cursor) {
            if child.kind() == "decorator" {
                result.push(state.node_text(child));
            }
        }
    }
    result
}

/// Inspects Angular/NestJS decorators on a class and emits framework symbols/edges.
pub fn classify_class_decorators(
    class_name: &str,
    decorators: &[&str],
    line: u32,
    state: &mut ExtractState<'_>,
) -> (SymbolKind, Option<String>) {
    for dec in decorators {
        if dec.starts_with("@Component") {
            return handle_angular_component(class_name, dec, line, state);
        }
        if dec.starts_with("@Module") {
            return handle_nest_module(class_name, dec, line, state);
        }
        if dec.starts_with("@Injectable") {
            return (SymbolKind::Class, Some("@Injectable".to_owned()));
        }
    }
    (SymbolKind::Class, None)
}

fn handle_angular_component(
    class_name: &str,
    dec: &str,
    line: u32,
    state: &mut ExtractState<'_>,
) -> (SymbolKind, Option<String>) {
    if let Some(selector) = extract_prop_string(dec, "selector") {
        state.push_symbol(
            selector.clone(),
            format!("{class_name}::{selector}"),
            SymbolKind::Component,
            line,
            Some("angular_selector".to_owned()),
            true,
        );
    }
    if let Some(tpl) = extract_prop_string(dec, "templateUrl") {
        state.push_edge(
            Some(class_name.to_owned()),
            tpl,
            EdgeKind::RendersTemplate,
            line,
        );
    }
    (SymbolKind::Component, Some("@Component".to_owned()))
}

fn handle_nest_module(
    class_name: &str,
    dec: &str,
    line: u32,
    state: &mut ExtractState<'_>,
) -> (SymbolKind, Option<String>) {
    extract_nest_array(dec, "imports", class_name, EdgeKind::Imports, line, state);
    extract_nest_array(
        dec,
        "providers",
        class_name,
        EdgeKind::Provides,
        line,
        state,
    );
    extract_nest_array(
        dec,
        "controllers",
        class_name,
        EdgeKind::Provides,
        line,
        state,
    );
    (SymbolKind::Module, Some("@Module".to_owned()))
}

fn extract_prop_string(decorator: &str, key: &str) -> Option<String> {
    let idx = decorator.find(key)?;
    let rest = &decorator[idx + key.len()..];
    let colon = rest.find(':')?;
    let after_colon = rest[colon + 1..].trim_start();
    let quote = after_colon.chars().next()?;
    if !matches!(quote, '\'' | '"' | '`') {
        return None;
    }
    let body = &after_colon[quote.len_utf8()..];
    let end = body.find(quote)?;
    Some(body[..end].to_owned())
}

fn extract_nest_array(
    decorator: &str,
    key: &str,
    class_name: &str,
    kind: EdgeKind,
    line: u32,
    state: &mut ExtractState<'_>,
) {
    let Some(idx) = decorator.find(key) else {
        return;
    };
    let rest = &decorator[idx + key.len()..];
    let Some(open) = rest.find('[') else {
        return;
    };
    let after_open = &rest[open + 1..];
    let Some(close) = after_open.find(']') else {
        return;
    };
    for item in after_open[..close].split(',') {
        let token = item.trim();
        if !token.is_empty() && token.chars().all(|c| c.is_alphanumeric() || c == '_') {
            state.push_edge(Some(class_name.to_owned()), token.to_owned(), kind, line);
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
                    state.push_edge(
                        class_name.map(str::to_owned),
                        text.to_owned(),
                        EdgeKind::Injects,
                        line,
                    );
                }
            }
        }
    }
}
