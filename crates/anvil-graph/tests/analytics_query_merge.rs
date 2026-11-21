//! Tests for query, path, explain, origin, analytics, multi-repo merge, freshness, and Graphify overlay.

use std::fs;
use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_core::traits::{FreshnessState, SymbolFreshnessProvider};
use anvil_graph::merge::merge_graphs;
use anvil_graph::model::NodeKind;
use anvil_graph::CodeGraphEngine;
use tempfile::TempDir;

fn write_repo_file(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(path, content).expect("write fixture file");
}

#[test]
fn query_path_explain_origin_analytics_and_freshness() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    write_repo_file(root, "src/core/auth.ts", "export function verifyToken() { return true; }\n");
    write_repo_file(root, "src/core/index.ts", "export * from './auth';\n");
    write_repo_file(
        root,
        "src/api/session.ts",
        "import { verifyToken } from '../core';\n\
         import { cycleB } from './cycleB';\n\
         export function createSession() { return verifyToken() && cycleB(); }\n",
    );
    write_repo_file(
        root,
        "src/api/cycleB.ts",
        "import { createSession } from './session';\n\
         export function cycleB() { return Boolean(createSession); }\n",
    );

    let mut engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build");

    let hits = engine.query("verifyToken", 5);
    assert!(!hits.is_empty());
    assert_eq!(hits[0].label, "verifyToken");

    let path = engine
        .shortest_path("src/api/session.ts", "src/core/auth.ts")
        .expect("path exists");
    assert_eq!(path.steps.len(), 2);

    let origin = engine.find_origin("src/core/index.ts").expect("origin");
    assert_eq!(origin.origin_file, "src/core/auth.ts");

    let communities = engine.detect_communities();
    assert!(!communities.is_empty());

    let explained = engine.explain("src/core/auth.ts").expect("explain");
    assert!(explained.community.is_some());
    assert!(!explained.incoming.is_empty());

    let cycles = engine.cycles(10);
    assert!(!cycles.is_empty(), "expected cycle between session.ts and cycleB.ts");

    let report = engine.report_markdown();
    assert!(report.contains("# Anvil Code Graph Report"));
    assert!(report.contains("Import Cycles"));

    let html_path = root.join(".harness/.state/graph.html");
    engine.export_html(&html_path).expect("export html");
    assert!(html_path.is_file());

    // SymbolFreshnessProvider checks
    let fresh = engine
        .check_freshness(&["src/core/auth.ts".to_owned()], &["verifyToken".to_owned()])
        .expect("freshness");
    assert_eq!(fresh.state, FreshnessState::Fresh);

    write_repo_file(root, "src/core/auth.ts", "export function verifyToken() { return false; }\n");
    let modified = engine
        .check_freshness(&["src/core/auth.ts".to_owned()], &["verifyToken".to_owned()])
        .expect("needs verification");
    assert_eq!(modified.state, FreshnessState::NeedsVerification);

    let stale = engine
        .check_freshness(&["src/core/missing.ts".to_owned()], &[])
        .expect("stale");
    assert_eq!(stale.state, FreshnessState::Stale);
}

#[test]
fn multi_repo_merge_and_graphify_overlay() {
    let front_dir = TempDir::new().expect("front");
    let back_dir = TempDir::new().expect("back");

    write_repo_file(
        front_dir.path(),
        "src/client.ts",
        "import { fetchOrders } from './api';\nexport function load() { return fetchOrders(); }\n",
    );
    write_repo_file(
        front_dir.path(),
        "src/api.ts",
        "export function fetchOrders() { return OrderDto(); }\n",
    );
    write_repo_file(
        back_dir.path(),
        "src/orders.ts",
        "export class OrderDto {}\n",
    );

    let mut front = CodeGraphEngine::build(front_dir.path(), AnvilConfig::default()).expect("front");
    let back = CodeGraphEngine::build(back_dir.path(), AnvilConfig::default()).expect("back");

    // Apply Graphify NetworkX overlay with semantic/rationale node & hyperedge
    let graphify_path = front_dir.path().join("graphify-out/graph.json");
    write_repo_file(
        front_dir.path(),
        "graphify-out/graph.json",
        r#"{
          "directed": false,
          "multigraph": false,
          "graph": {
            "hyperedges": [
              {
                "id": "orders_flow",
                "label": "Orders End-to-End Flow",
                "nodes": ["client_ts"],
                "source_file": "src/client.ts"
              }
            ]
          },
          "nodes": [
            { "id": "client_ts", "label": "client.ts", "file_type": "code", "source_file": "src/client.ts" },
            { "id": "adr_orders", "label": "Orders Caching Rationale", "file_type": "rationale", "source_file": "src/api.ts", "_origin": "llm" }
          ],
          "links": [
            { "source": "adr_orders", "target": "client_ts", "relation": "explains" }
          ]
        }"#,
    );
    let overlay = front.apply_graphify_overlay(&graphify_path).expect("overlay");
    assert_eq!(overlay.semantic_nodes_added, 1);
    assert!(overlay.hyperedges_linked >= 1);
    assert!(
        front
            .graph
            .nodes
            .values()
            .any(|n| n.kind == NodeKind::Semantic && n.label == "Orders Caching Rationale")
    );

    let merged = merge_graphs(&[("fliiper-front", &front.graph), ("fliiper-back", &back.graph)]);
    assert!(merged.nodes.contains_key("file:fliiper-front/src/client.ts"));
    assert!(merged.nodes.contains_key("file:fliiper-back/src/orders.ts"));
}
