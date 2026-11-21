//! Tier 0 rule: detect pure re-export barrel files (`index.{ts,tsx,js,jsx}`).

use anvil_core::traits::{Diagnostic, Language, Severity};
use tree_sitter::Node;

use super::{RuleState, has_anvil_allow, is_comment_kind};

/// Flags `index.{ts,tsx,js,jsx}` files that contain only `export ... from ...` re-exports.
pub fn check_barrel(root: Node<'_>, state: &mut RuleState<'_>) {
    if !matches!(
        state.language,
        Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Jsx
    ) {
        return;
    }

    let Some(file_name) = state.path.file_name().and_then(|n| n.to_str()) else {
        return;
    };
    if !matches!(
        file_name.to_ascii_lowercase().as_str(),
        "index.ts" | "index.tsx" | "index.js" | "index.jsx"
    ) {
        return;
    }

    let mut reexport_count = 0usize;
    let mut has_local_logic = false;
    let mut first_reexport_line = 1u32;

    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        let kind = child.kind();
        if is_comment_kind(kind) || matches!(kind, "empty_statement" | "hash_bang_line") {
            continue;
        }
        if kind == "export_statement" && child.child_by_field_name("source").is_some() {
            if reexport_count == 0 {
                first_reexport_line = child.start_position().row as u32 + 1;
            }
            reexport_count += 1;
        } else {
            has_local_logic = true;
            break;
        }
    }

    if reexport_count > 0 && !has_local_logic && !has_anvil_allow(state, 1, "no-barrel-files") {
        state.diagnostics.push(Diagnostic {
            rule: "no-barrel-files".to_owned(),
            severity: Severity::Error,
            file: state.file.to_owned(),
            line: first_reexport_line,
            column: 1,
            message: format!(
                "Pure re-export barrel `{file_name}` obscures dependencies and slows tree-shaking"
            ),
            suggestion: Some(
                "Import directly from source modules instead of re-exporting via index files"
                    .to_owned(),
            ),
        });
    }
}
