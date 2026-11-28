use std::fs;

use anvil_core::traits::SymbolKind;
use anvil_docs::concept;
use anvil_docs::create::{NewConcept, create_concept};
use anvil_docs::markdown_edit::add_log_entry;
use anvil_docs::outline::{self, END, START};
use anvil_docs::paths::PathMatcher;
use anvil_docs::refresh::refresh_concept;
use anvil_docs::search::search;
use anvil_graph::model::{CodeGraph, GraphNode, NodeKind};

use crate::common::{bundle, repo, write};

fn graph_with(symbols: &[(&str, &str, u32)]) -> CodeGraph {
    let mut graph = CodeGraph::new("repo");
    for (file, name, line) in symbols {
        graph.nodes.push(GraphNode {
            id: format!("sym:{file}::{name}"),
            kind: NodeKind::Symbol,
            label: (*name).to_owned(),
            file: (*file).to_owned(),
            line: *line,
            end_line: *line,
            language: None,
            symbol_kind: Some(SymbolKind::Function),
            signature: None,
            exported: true,
            community: None,
        });
    }
    graph
}

#[test]
fn outline_lists_exported_symbols_of_matching_files_only() {
    let graph =
        graph_with(&[("src/orders/service.ts", "createOrder", 3), ("src/billing/a.ts", "bill", 1)]);
    let matcher = PathMatcher::new(&["src/orders/**".to_owned()]).expect("glob");
    let text = outline::render(&graph, &matcher);
    assert_eq!(text, "- `function` `createOrder` - `src/orders/service.ts:3`\n");
}

#[test]
fn outline_is_capped() {
    let symbols: Vec<(&str, String, u32)> =
        (0..70).map(|i| ("src/orders/a.ts", format!("f{i:02}"), i + 1)).collect();
    let refs: Vec<(&str, &str, u32)> =
        symbols.iter().map(|(f, n, l)| (*f, n.as_str(), *l)).collect();
    let matcher = PathMatcher::new(&["src/orders/**".to_owned()]).expect("glob");
    let text = outline::render(&graph_with(&refs), &matcher);
    assert_eq!(text.lines().count(), outline::MAX_SYMBOLS + 1);
    assert!(text.ends_with("- ... and 10 more exported symbols\n"));
}

#[test]
fn refresh_regenerates_only_the_outline_block() {
    let dir = repo();
    let root = dir.path();
    let bundle = bundle(root);
    let spec = NewConcept {
        module: "orders".to_owned(),
        paths: vec!["src/orders/**".to_owned()],
        ..NewConcept::default()
    };
    let file = create_concept(root, &bundle, &spec, None).expect("create");
    let created = fs::read_to_string(&file).expect("read");
    assert!(created.contains(START) && created.contains(END));
    assert!(!created.contains("TODO") && !created.contains("FIXME"));

    // The author edits around the block, including odd whitespace and CRLF.
    let authored = created
        .replace("## Purpose\n\n", "## Purpose\n\nHandles orders.  \r\n\twith tabs\n\n")
        .replace("## Invariants\n", "## Invariants\n\n- ids are unique\n");
    fs::write(&file, &authored).expect("write");

    let graph = graph_with(&[("src/orders/service.ts", "createOrder", 1)]);
    let loaded = concept::scan(root, &bundle, true).concepts.remove(0);
    let done = refresh_concept(root, &bundle, &loaded, Some(&graph)).expect("refresh");
    assert!(done.outline_updated);

    let after = fs::read_to_string(&file).expect("read");
    let (before_head, before_tail) = split_block(&authored);
    let (after_head, after_tail) = split_block(&after);
    let strip = |text: &str| text.split("---\n").nth(2).map(str::to_owned).unwrap_or_default();
    assert_eq!(strip(&after_head), strip(&before_head), "text before the block changed");
    assert_eq!(after_tail, before_tail, "text after the block changed");
    assert!(after.contains("`createOrder` - `src/orders/service.ts:1`"));
    assert!(after.contains("generated:"));

    // A second refresh with the same graph leaves the body untouched.
    let again = concept::scan(root, &bundle, true).concepts.remove(0);
    refresh_concept(root, &bundle, &again, Some(&graph)).expect("refresh again");
    let body_of = |text: &str| text.split("---\n").skip(2).collect::<Vec<_>>().join("---\n");
    assert_eq!(body_of(&fs::read_to_string(&file).expect("read")), body_of(&after));
}

fn split_block(text: &str) -> (String, String) {
    let start = text.find(START).expect("start marker") + START.len();
    let end = text.find(END).expect("end marker");
    (text[..start].to_owned(), text[end..].to_owned())
}

#[test]
fn search_ranks_by_relevance() {
    let dir = repo();
    let root = dir.path();
    let bundle = bundle(root);
    for (module, description, extra) in [
        ("orders", "Order placement", "Creates orders and reserves stock."),
        ("billing", "Invoices and payments", "Sends the invoice after payment. Invoice numbering."),
    ] {
        let spec = NewConcept {
            module: module.to_owned(),
            paths: vec![format!("src/{module}/**")],
            description: Some(description.to_owned()),
            ..NewConcept::default()
        };
        let file = create_concept(root, &bundle, &spec, None).expect("create");
        let mut text = fs::read_to_string(&file).expect("read");
        text.push_str(&format!("\n{extra}\n"));
        fs::write(&file, text).expect("write");
    }
    write(root, ".harness/docs/shop/notes.md", "# loose notes about invoice\n");
    let concepts = concept::scan_all(root, &[bundle], true);
    let hits = search(&concepts, "invoice", 5);
    assert_eq!(hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["shop/billing"]);
    assert!(hits[0].snippet.to_lowercase().contains("invoice"));
    assert!(hits[0].path.ends_with("billing/overview.md"));
    let both = search(&concepts, "orders stock invoice", 1);
    assert_eq!(both.len(), 1);
    assert!(search(&concepts, "zebra", 5).is_empty());
    assert!(search(&concepts, "   ", 5).is_empty());
}

#[test]
fn log_entries_group_by_date_newest_first() {
    let base = "# Log\n\nHistory.\n\n## 2026-01-01\n\n- Old entry.\n";
    let today = add_log_entry(base, "2026-02-02", "New entry.");
    assert_eq!(
        today,
        "# Log\n\nHistory.\n\n## 2026-02-02\n\n- New entry.\n\n## 2026-01-01\n\n- Old entry.\n"
    );
    let same_day = add_log_entry(&today, "2026-02-02", "Second entry.");
    assert!(
        same_day.contains("## 2026-02-02\n\n- New entry.\n- Second entry.\n\n## 2026-01-01"),
        "{same_day}"
    );
    let empty = add_log_entry("# Log\n", "2026-03-03", "First.");
    assert_eq!(empty, "# Log\n\n## 2026-03-03\n\n- First.\n");
}
