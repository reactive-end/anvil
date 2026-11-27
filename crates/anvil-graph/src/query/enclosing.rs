//! Maps a source line to its innermost enclosing symbol and annotates grep reports with it.

use anvil_core::diff_grep::GrepReport;
use anvil_core::traits::SymbolKind;

use crate::model::{CodeGraph, GraphNode, NodeKind};

/// Returns the innermost Symbol node of `rel` whose `line..=end_line` span contains `line`.
///
/// `rel` may use either path separator. Ties on span size resolve to the symbol that starts later.
pub fn enclosing_symbol<'g>(graph: &'g CodeGraph, rel: &str, line: u32) -> Option<&'g GraphNode> {
    let norm = CodeGraph::normalize_rel_path(rel);
    let ids = graph.file_nodes.get(&norm)?;
    ids.iter()
        .filter_map(|&id| graph.get_node(id))
        .filter(|n| n.kind == NodeKind::Symbol && n.line <= line && line <= n.end_line.max(n.line))
        .min_by_key(|n| (n.end_line.max(n.line) - n.line, std::cmp::Reverse(n.line)))
}

/// Short human label of a symbol: kind prefix plus name, e.g. `fn find_user` or `class OrdersService`.
pub fn symbol_label(node: &GraphNode) -> String {
    let prefix = match node.symbol_kind {
        Some(SymbolKind::Function) => "fn",
        Some(SymbolKind::Class) => "class",
        Some(SymbolKind::Interface) => "interface",
        Some(SymbolKind::TypeAlias) => "type",
        Some(SymbolKind::Component) => "component",
        Some(SymbolKind::Module) => "mod",
        Some(SymbolKind::Method) => "method",
        Some(SymbolKind::Variable) => "var",
        Some(SymbolKind::Constant) => "const",
        Some(SymbolKind::Unknown) | None => return node.label.clone(),
    };
    format!("{prefix} {}", node.label)
}

/// Fills `LineMatch::symbol` for every shown match of `report` using `graph`.
pub fn annotate_grep_report(graph: &CodeGraph, report: &mut GrepReport) {
    for file in &mut report.files {
        for m in &mut file.matches {
            m.symbol = enclosing_symbol(graph, &file.path, m.line).map(symbol_label);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use anvil_core::config::AnvilConfig;
    use anvil_core::diff_grep::{FileMatches, GrepReport, LineMatch};
    use tempfile::TempDir;

    use super::*;
    use crate::CodeGraphEngine;

    const SERVICE: &str = "export class OrdersService {\n  findOrder(id: string) {\n    const needle = id;\n    return needle;\n  }\n\n  cancel(id: string) {\n    return id;\n  }\n}\n\nexport function helper() {\n  return 1;\n}\n";

    fn graph_for_fixture() -> (TempDir, CodeGraph) {
        let dir = TempDir::new().expect("tempdir");
        fs::create_dir_all(dir.path().join("src")).expect("src dir");
        fs::write(dir.path().join("src/orders.ts"), SERVICE).expect("write fixture");
        let engine = CodeGraphEngine::build(dir.path(), AnvilConfig::default()).expect("build");
        (dir, engine.graph)
    }

    fn label_at(graph: &CodeGraph, rel: &str, line: u32) -> Option<String> {
        enclosing_symbol(graph, rel, line).map(symbol_label)
    }

    #[test]
    fn picks_the_innermost_symbol_and_none_outside_any() {
        let (_dir, graph) = graph_for_fixture();
        let inner = enclosing_symbol(&graph, "src/orders.ts", 3).expect("symbol on line 3");
        assert_eq!(inner.label, "findOrder", "method wins over the enclosing class");
        assert!(inner.end_line - inner.line < 5);

        assert_eq!(label_at(&graph, "src/orders.ts", 1).as_deref(), Some("class OrdersService"));
        assert_eq!(
            label_at(&graph, "src/orders.ts", 8).as_deref().map(|l| l.ends_with("cancel")),
            Some(true)
        );
        assert_eq!(label_at(&graph, "src/orders.ts", 13).as_deref(), Some("fn helper"));
        assert_eq!(
            label_at(&graph, "src/orders.ts", 11),
            None,
            "blank line between top-level symbols"
        );
        assert_eq!(label_at(&graph, "src/missing.ts", 3), None);
    }

    #[test]
    fn normalizes_path_separators() {
        let (_dir, graph) = graph_for_fixture();
        assert_eq!(label_at(&graph, r".\src\orders.ts", 13).as_deref(), Some("fn helper"));
        assert_eq!(label_at(&graph, "./src/orders.ts", 13).as_deref(), Some("fn helper"));
    }

    #[test]
    fn annotates_report_matches_with_symbols() {
        let (_dir, graph) = graph_for_fixture();
        let line = |n| LineMatch {
            line: n,
            text: "x".into(),
            before: vec![],
            after: vec![],
            symbol: None,
        };
        let mut report = GrepReport {
            files: vec![FileMatches {
                path: "src/orders.ts".into(),
                matches: vec![line(3), line(13), line(11)],
                total: 3,
            }],
            total_matches: 3,
            files_with_matches: 1,
            files_searched: 1,
            truncated: false,
            backend: "builtin".into(),
            note: None,
            estimated_tokens_saved: 0,
        };
        annotate_grep_report(&graph, &mut report);
        let symbols: Vec<Option<&str>> =
            report.files[0].matches.iter().map(|m| m.symbol.as_deref()).collect();
        assert_eq!(symbols[1], Some("fn helper"));
        assert!(symbols[0].is_some_and(|s| s.ends_with("findOrder")), "{symbols:?}");
        assert_eq!(symbols[2], None);
    }
}
