//! Clean call-target extraction for TypeScript and JavaScript ASTs.

use tree_sitter::Node;

use crate::extractors::ExtractState;

/// Extracts a concise single-line callee identifier or member path (`foo`, `svc.fetch`),
/// avoiding multi-line expression dumps on chained calls.
pub fn clean_callee(node: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    let candidate = match node.kind() {
        "identifier" | "private_property_identifier" | "this" | "super" => {
            state.node_text(node).trim().to_owned()
        }
        "member_expression" => {
            let prop = node.child_by_field_name("property")?;
            let prop_name = state.node_text(prop).trim();
            if prop_name.is_empty() {
                return None;
            }
            if let Some(obj) = node.child_by_field_name("object")
                && let Some(obj_name) = clean_member_object(obj, state)
            {
                let combined = format!("{obj_name}.{prop_name}");
                if combined.len() <= 80 {
                    combined
                } else {
                    prop_name.to_owned()
                }
            } else {
                prop_name.to_owned()
            }
        }
        "non_null_expression"
        | "as_expression"
        | "type_assertion"
        | "satisfies_expression"
        | "parenthesized_expression" => {
            let inner = node.named_child(0)?;
            return clean_callee(inner, state);
        }
        _ => return None,
    };

    ExtractState::sanitize_call_target(candidate)
}

fn clean_member_object(node: Node<'_>, state: &ExtractState<'_>) -> Option<String> {
    match node.kind() {
        "identifier" | "this" | "super" => {
            ExtractState::sanitize_call_target(state.node_text(node).trim().to_owned())
        }
        "member_expression" => {
            let obj = node.child_by_field_name("object")?;
            let prop = node.child_by_field_name("property")?;
            let obj_name = clean_member_object(obj, state)?;
            let prop_name = state.node_text(prop).trim();
            ExtractState::sanitize_call_target(format!("{obj_name}.{prop_name}"))
        }
        _ => None,
    }
}
