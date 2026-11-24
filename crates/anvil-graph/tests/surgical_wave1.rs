//! Regression tests for Surgical Wave S1 (`anvil-graph`).

use std::fs;
use std::path::Path;
use std::time::Instant;

use anvil_core::config::AnvilConfig;
use anvil_graph::impact::filter_typecheck_output;
use anvil_graph::resolvers::tsconfig::TsConfigRules;
use anvil_graph::{CodeGraphEngine, store};
use tempfile::TempDir;

fn write_repo_file(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(path, content).expect("write fixture file");
}

#[test]
fn store_read_header_returns_exact_counts_and_rejects_corrupt() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    write_repo_file(root, "src/a.ts", "export function alpha() { return 1; }\n");
    write_repo_file(root, "src/b.ts", "export function beta() { return 2; }\n");

    let engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build");
    let bin_path = engine.save().expect("save initial graph");

    let _ = store::read_header(&bin_path).expect("warmup read header");
    let t0 = Instant::now();
    let hdr = store::read_header(&bin_path).expect("read header");
    let elapsed_ms = t0.elapsed().as_secs_f64() * 1000.0;

    let budget_ms = if cfg!(debug_assertions) { 5.0 } else { 1.0 };
    assert!(elapsed_ms < budget_ms, "read_header took {elapsed_ms:.3}ms (budget {budget_ms}ms)");
    assert_eq!(hdr.version, 3);
    assert_eq!(hdr.file_count, 2);
    assert_eq!(hdr.live_nodes as usize, engine.graph.nodes.len());
    assert_eq!(hdr.live_edges as usize, engine.graph.edges.len());

    let bad_magic = root.join("bad_magic.bin");
    fs::write(&bad_magic, b"BADMAGIC\x02\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00")
        .expect("write bad magic");
    assert!(store::read_header(&bad_magic).is_err());

    let bad_ver = root.join("bad_ver.bin");
    fs::write(&bad_ver, b"ANVLGRPH\x01\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00")
        .expect("write bad version");
    assert!(store::read_header(&bad_ver).is_err());

    let truncated = root.join("truncated.bin");
    fs::write(&truncated, b"ANVLGRPH").expect("write truncated");
    assert!(store::read_header(&truncated).is_err());
}

#[test]
fn store_recomputes_tombstones_on_reload_and_triggers_compaction() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    write_repo_file(root, "src/a.ts", "export function alpha() { return 1; }\n");
    write_repo_file(root, "src/b.ts", "export function beta() { return 2; }\n");
    write_repo_file(root, "src/c.ts", "export function gamma() { return 3; }\n");
    write_repo_file(root, "src/d.ts", "export function delta() { return 4; }\n");

    let mut engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build");
    let bin_path = engine.save().expect("save initial graph");

    // Delete 1 file (below 25% compaction threshold), save, and reload in a fresh graph.
    fs::remove_file(root.join("src/d.ts")).expect("remove d.ts");
    assert!(engine.update_file("src/d.ts").expect("remove d.ts from graph"));
    assert!(engine.graph.tombstone_nodes > 0);
    assert!(engine.graph.tombstone_edges > 0);
    let expected_live_nodes = engine.graph.nodes.iter().filter(|n| !n.is_tombstone()).count();
    let expected_live_edges = engine.graph.edges.iter().filter(|e| !e.is_tombstone()).count();

    engine.save().expect("save graph with tombstones");
    let hdr_after = store::read_header(&bin_path).expect("read header after tombstone");
    assert_eq!(hdr_after.file_count, 3);
    assert_eq!(hdr_after.live_nodes as usize, expected_live_nodes);
    assert_eq!(hdr_after.live_edges as usize, expected_live_edges);

    // Tombstone counts must be recomputed on reload so subsequent 25% compaction triggers.
    let mut reloaded = store::load_binary(&bin_path).expect("reload binary");
    assert_eq!(reloaded.tombstone_nodes, engine.graph.tombstone_nodes);
    assert_eq!(reloaded.tombstone_edges, engine.graph.tombstone_edges);

    fs::remove_file(root.join("src/c.ts")).expect("remove c.ts");
    let _ = anvil_graph::builder::update_file(&mut reloaded, root, "src/c.ts", &AnvilConfig::default())
        .expect("remove c.ts");
    assert_eq!(
        reloaded.tombstone_nodes, 0,
        "25% tombstone compaction must trigger after reload"
    );
}

#[test]
fn tsconfig_precedence_and_node_modules_extends() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    write_repo_file(
        root,
        "node_modules/@tsconfig/node20/tsconfig.json",
        r#"{ "compilerOptions": { "paths": { "@nm/*": ["src/from-nm/*"], "@override/*": ["src/wrong/*"] } } }"#,
    );
    write_repo_file(
        root,
        "node_modules/expo/tsconfig.base.json",
        r#"{ "extends": "@tsconfig/node20/tsconfig.json", "compilerOptions": { "paths": { "@expo-alias/*": ["src/expo/*"] } } }"#,
    );
    write_repo_file(
        root,
        "tsconfig.base.json",
        r#"{
          "compilerOptions": {
            "baseUrl": "./legacy",
            "paths": {
              "@app/*": ["src/legacy-app/*"],
              "@shared/*": ["src/shared/*"]
            }
          }
        }"#,
    );
    write_repo_file(
        root,
        "tsconfig.json",
        r#"{
          "extends": "expo/tsconfig.base",
          "compilerOptions": {
            "baseUrl": ".",
            "paths": {
              "@app/*": ["src/real-app/*"],
              "@override/*": ["src/right/*"]
            }
          }
        }"#,
    );

    let rules = TsConfigRules::load(root);
    assert_eq!(rules.base_url.as_deref(), Some(""));

    let app_targets = rules
        .paths
        .iter()
        .find(|(p, _)| p == "@app/*")
        .map(|(_, t)| t.as_slice());
    assert_eq!(app_targets, Some(&["src/real-app/*".to_owned()][..]));

    let override_targets = rules
        .paths
        .iter()
        .find(|(p, _)| p == "@override/*")
        .map(|(_, t)| t.as_slice());
    assert_eq!(override_targets, Some(&["src/right/*".to_owned()][..]));

    assert!(rules.paths.iter().any(|(p, _)| p == "@nm/*"));
    assert!(rules.paths.iter().any(|(p, _)| p == "@expo-alias/*"));
    assert!(rules.paths.iter().any(|(p, _)| p == "@shared/*"));

    // End-to-end blast radius uses the overridden @app/* target, not legacy-app!
    write_repo_file(root, "src/legacy-app/svc.ts", "export const x = 1;\n");
    write_repo_file(root, "src/real-app/svc.ts", "export const x = 2;\n");
    write_repo_file(
        root,
        "src/consumer.ts",
        "import { x } from '@app/svc';\nexport const y = x;\n",
    );

    let engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build");
    let real_blast = engine.blast_radius(&["src/real-app/svc.ts".to_owned()], 2);
    assert!(real_blast.impacted_files.iter().any(|i| i.file == "src/consumer.ts"));
    let legacy_blast = engine.blast_radius(&["src/legacy-app/svc.ts".to_owned()], 2);
    assert!(!legacy_blast.impacted_files.iter().any(|i| i.file == "src/consumer.ts"));
}

#[test]
fn typecheck_filter_preserves_exit0_warnings_and_boundary_matches_paths() {
    // 1. exit_code == 0 with dart analyze info / pyright warning on impacted file MUST PASS.
    let dart_info = "   info - Unused import - lib/clean.dart:1:8 - unused_import\n";
    let dart_out = filter_typecheck_output(
        None,
        "dart analyze",
        0,
        dart_info,
        &["lib/clean.dart".to_owned()],
        50,
        1200,
    )
    .expect("dart filter");
    assert!(dart_out.passed, "exit 0 info must never become failure: {dart_out:?}");
    assert_eq!(dart_out.exit_code, 0);

    let pyright_warn = "  src/app.py:4:5 - warning: Variable unused\n";
    let py_out = filter_typecheck_output(
        None,
        "pyright",
        0,
        pyright_warn,
        &["src/app.py".to_owned()],
        50,
        1200,
    )
    .expect("pyright filter");
    assert!(py_out.passed, "exit 0 warning must stay PASS");
    assert_eq!(py_out.exit_code, 0);

    // 2. Boundary matching: src/a.ts MUST NOT match src/a.tsx or other_src/a.ts!
    let tsx_error = "\
src/a.tsx(10,2): error TS2322: Type 'string' is not assignable.\n\
other_src/a.ts(3,1): error TS2304: Cannot find name 'X'.\n";
    let boundary_out = filter_typecheck_output(
        None,
        "tsc --noEmit",
        2,
        tsx_error,
        &["src/a.ts".to_owned()],
        50,
        1200,
    )
    .expect("boundary filter");
    assert!(boundary_out.passed, "src/a.tsx must not match src/a.ts");
    assert_eq!(boundary_out.total_errors, 0);

    // Exact match on src/a.ts still fails properly when exit_code != 0.
    let ts_error = "src/a.ts(10,2): error TS2322: Type 'string' is not assignable.\n";
    let match_out = filter_typecheck_output(
        None,
        "tsc --noEmit",
        2,
        ts_error,
        &["src/a.ts".to_owned()],
        50,
        1200,
    )
    .expect("exact match filter");
    assert!(!match_out.passed);
    assert_eq!(match_out.total_errors, 1);
}

#[test]
fn typecheck_filter_preserves_unattributed_and_multiline_rust_errors() {
    let global_err = "error TS5023: Unknown compiler option '--bogus'.\n";
    let global_out = filter_typecheck_output(
        None,
        "tsc --noEmit",
        2,
        global_err,
        &["src/a.ts".to_owned()],
        50,
        1200,
    )
    .expect("global failure");
    assert!(!global_out.passed, "unattributed compiler failure must fail: {global_out:?}");
    assert_eq!(global_out.exit_code, 2);

    let rust_impacted = "error[E0308]: mismatched types\n  --> src/lib.rs:10:5\n   |\n10 |     \"bad\"\n";
    let rust_fail = filter_typecheck_output(
        None,
        "cargo check",
        101,
        rust_impacted,
        &["src/lib.rs".to_owned()],
        50,
        1200,
    )
    .expect("rust impacted error");
    assert!(!rust_fail.passed, "Rust multi-line error on impacted file must fail");
    assert!(rust_fail.lines.iter().any(|l| l.contains("E0308")));

    let rust_other = "error[E0308]: mismatched types\n  --> src/unrelated.rs:10:5\n";
    let rust_ok = filter_typecheck_output(
        None,
        "cargo check",
        101,
        rust_other,
        &["src/lib.rs".to_owned()],
        50,
        1200,
    )
    .expect("rust unrelated error");
    assert!(rust_ok.passed, "Rust multi-line error strictly on unrelated file may be suppressed");
}

#[test]
fn sibling_tests_are_directory_scoped_and_rust_barrels_trace_origin() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    // Sibling scoping: pkg-a vs pkg-b
    write_repo_file(root, "pkg-a/utils.ts", "export function fmt() { return 'a'; }\n");
    write_repo_file(root, "pkg-a/utils.test.ts", "export const aTest = 1;\n");
    write_repo_file(root, "pkg-a/__tests__/utils.test.ts", "export const aSubTest = 1;\n");
    write_repo_file(root, "pkg-b/utils.ts", "export function fmt() { return 'b'; }\n");
    write_repo_file(root, "pkg-b/utils.test.ts", "export const bTest = 1;\n");

    // Rust barrel origin via lib.rs and mod.rs
    write_repo_file(root, "Cargo.toml", "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n");
    write_repo_file(root, "src/lib.rs", "pub mod core;\npub use core::engine::run_engine;\n");
    write_repo_file(root, "src/core/mod.rs", "pub mod engine;\npub use engine::run_engine;\n");
    write_repo_file(root, "src/core/engine.rs", "pub fn run_engine() -> i32 { 42 }\n");

    let engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build");

    let blast = engine.blast_radius(&["pkg-a/utils.ts".to_owned()], 2);
    assert!(blast.related_tests.contains(&"pkg-a/utils.test.ts".to_owned()));
    assert!(blast.related_tests.contains(&"pkg-a/__tests__/utils.test.ts".to_owned()));
    assert!(
        !blast.related_tests.contains(&"pkg-b/utils.test.ts".to_owned()),
        "pkg-a/utils.ts must never drag in pkg-b/utils.test.ts: {:?}",
        blast.related_tests
    );

    let origin = engine
        .find_origin("src/lib.rs::run_engine")
        .expect("origin through Rust lib.rs/mod.rs barrels");
    assert_eq!(origin.origin_file, "src/core/engine.rs");
    assert_eq!(origin.origin_label, "run_engine");
}

#[test]
fn load_cached_rebuilds_cleanly_on_legacy_v2_header() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write_repo_file(
        root,
        "src/lib.ts",
        "export function greet(name: string) {\n  return `hi ${name}`;\n}\n",
    );

    let bin_path = store::default_bin_path(root);
    fs::create_dir_all(bin_path.parent().expect("parent")).expect("mkdir");
    fs::write(
        &bin_path,
        b"ANVLGRPH\x02\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00",
    )
    .expect("write v2 header");
    assert!(store::read_header(&bin_path).is_err());

    let engine = CodeGraphEngine::load_cached(root, AnvilConfig::default()).expect("rebuild v3");
    let slice = engine
        .read_symbol("greet", false)
        .expect("read_symbol")
        .expect("greet found");
    assert_eq!(slice.start_line, 1);
    assert_eq!(slice.end_line, 3);
}
