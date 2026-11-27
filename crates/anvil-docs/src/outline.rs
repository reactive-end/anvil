//! The generated outline block of a concept's "Public interface" section.

use anvil_graph::model::{CodeGraph, NodeKind};

use crate::paths::PathMatcher;

/// Opening marker of the generated block.
pub const START: &str = "<!-- anvil:outline:start -->";
/// Closing marker of the generated block.
pub const END: &str = "<!-- anvil:outline:end -->";
/// Most symbols listed before the rest is summarised.
pub const MAX_SYMBOLS: usize = 60;

/// Hint shown while no graph is available.
pub const NO_GRAPH_HINT: &str =
    "_No code graph yet: run `anvil graph build`, then `anvil docs refresh`._\n";

/// Lines of the exported symbols of the files matched by `matcher`, sorted by file and line.
pub fn render(graph: &CodeGraph, matcher: &PathMatcher) -> String {
    let mut symbols: Vec<(&str, u32, String)> = graph
        .nodes
        .iter()
        .filter(|n| n.kind == NodeKind::Symbol && n.exported && !n.is_tombstone())
        .filter(|n| matcher.is_match(&n.file))
        .map(|n| (n.file.as_str(), n.line, describe(n.symbol_kind, &n.label)))
        .collect();
    symbols.sort();
    symbols.dedup();
    if symbols.is_empty() {
        return "_No exported symbols found._\n".to_owned();
    }
    let mut out = String::new();
    for (file, line, text) in symbols.iter().take(MAX_SYMBOLS) {
        out.push_str(&format!("- {text} - `{file}:{line}`\n"));
    }
    if symbols.len() > MAX_SYMBOLS {
        out.push_str(&format!("- ... and {} more exported symbols\n", symbols.len() - MAX_SYMBOLS));
    }
    out
}

fn describe(kind: Option<anvil_core::traits::SymbolKind>, label: &str) -> String {
    let kind = kind.map_or_else(|| "symbol".to_owned(), |k| format!("{k:?}").to_ascii_lowercase());
    format!("`{kind}` `{label}`")
}

/// A complete block (markers included) holding `inner`, which must end with a newline.
pub fn block(inner: &str) -> String {
    format!("{START}\n{inner}{END}")
}

/// Replaces what lies between the markers with `inner`; the rest of `body` is untouched. Returns
/// `None` when the body has no complete block.
pub fn replace(body: &str, inner: &str) -> Option<String> {
    let start = body.find(START)?;
    let after_start = start + START.len();
    let end = after_start + body[after_start..].find(END)?;
    Some(format!("{}\n{inner}{}", &body[..after_start], &body[end..]))
}
