//! AST edge extractor for HTML and Angular component templates.

use anvil_core::traits::EdgeKind;
use tree_sitter::Node;

use super::ExtractState;

/// Visits a single HTML AST node to extract Angular component tags, bindings, and calls.
pub fn visit_node(node: Node<'_>, state: &mut ExtractState<'_>) {
    if !matches!(node.kind(), "start_tag" | "self_closing_tag") {
        return;
    }

    let mut tag_name = None;
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        if child.kind() == "tag_name" {
            let name = state.node_text(child).trim().to_owned();
            let line = ExtractState::line_of(child);
            if name.contains('-') {
                state.push_edge(None, name.clone(), EdgeKind::RendersTemplate, line);
            }
            tag_name = Some(name);
            break;
        }
    }

    let Some(tag) = tag_name else {
        return;
    };
    extract_template_attributes(node, &tag, state);
}

fn extract_template_attributes(tag_node: Node<'_>, tag: &str, state: &mut ExtractState<'_>) {
    let mut cursor = tag_node.walk();
    for child in tag_node.named_children(&mut cursor) {
        if child.kind() != "attribute" {
            continue;
        }
        let mut attr_name = None;
        let mut attr_val = None;
        let mut a_cursor = child.walk();
        for part in child.named_children(&mut a_cursor) {
            match part.kind() {
                "attribute_name" => attr_name = Some(state.node_text(part).trim()),
                "quoted_attribute_value" | "attribute_value" => {
                    attr_val = Some(ExtractState::unquote(state.node_text(part)));
                }
                _ => {}
            }
        }

        let Some(name) = attr_name else {
            continue;
        };
        let line = ExtractState::line_of(child);

        if tag.contains('-')
            && let Some(prop) = name.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
            && !prop.is_empty()
        {
            state.push_edge(None, format!("{tag}.{prop}"), EdgeKind::UsesProp, line);
        }

        if name.starts_with('(')
            && name.ends_with(')')
            && let Some(val) = attr_val
            && let Some(callee) = val.split('(').next().map(str::trim)
            && is_ident_chain(callee)
        {
            state.push_edge(None, callee.to_owned(), EdgeKind::Calls, line);
        }
    }
}

fn is_ident_chain(s: &str) -> bool {
    !s.is_empty()
        && s.split('.')
            .all(|seg| !seg.is_empty() && seg.chars().all(|c| c.is_alphanumeric() || c == '_'))
}
