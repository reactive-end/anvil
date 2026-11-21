//! Regression tests for Wave H3 hardening: Rust & CSS resolvers, Gradle subprojects,
//! multi-export barrel `find_origin`, `(mtime, size)` fast-path, `load_cached`, merge filtering,
//! and Windows `typecheck` error filtering.

use std::fs;
use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_graph::impact::filter_typecheck_output;
use anvil_graph::merge::merge_graphs;
use anvil_graph::model::GraphEdgeKind;
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
fn rust_mod_use_crate_super_and_workspace_resolution() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    write_repo_file(
        root,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n",
    );
    write_repo_file(
        root,
        "crates/core-lib/Cargo.toml",
        "[package]\nname = \"core-lib\"\nversion = \"0.1.0\"\n",
    );
    write_repo_file(
        root,
        "crates/core-lib/src/lib.rs",
        "pub mod config;\n",
    );
    write_repo_file(
        root,
        "crates/core-lib/src/config.rs",
        "pub struct Settings { pub enabled: bool }\n",
    );
    write_repo_file(
        root,
        "crates/app-bin/Cargo.toml",
        "[package]\nname = \"app-bin\"\nversion = \"0.1.0\"\n",
    );
    write_repo_file(
        root,
        "crates/app-bin/src/lib.rs",
        "pub mod handlers;\n",
    );
    write_repo_file(
        root,
        "crates/app-bin/src/handlers.rs",
        "mod sub;\nuse core_lib::config::Settings;\npub fn root_handler() {}\n",
    );
    write_repo_file(
        root,
        "crates/app-bin/src/handlers/sub.rs",
        "use crate::handlers::root_handler;\nuse super::root_handler as super_h;\npub fn sub_fn() { root_handler(); super_h(); }\n",
    );

    let engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build rust graph");

    // Changing core-lib/src/config.rs must impact app-bin/src/handlers.rs and core-lib/src/lib.rs
    let cfg_blast = engine.blast_radius(&["crates/core-lib/src/config.rs".to_owned()], 3);
    let impacted: Vec<&str> = cfg_blast.impacted_files.iter().map(|i| i.file.as_str()).collect();
    assert!(
        impacted.contains(&"crates/app-bin/src/handlers.rs"),
        "expected workspace crate consumer handlers.rs in {impacted:?}"
    );
    assert!(
        impacted.contains(&"crates/core-lib/src/lib.rs"),
        "expected mod config consumer lib.rs in {impacted:?}"
    );

    // Changing handlers.rs must impact handlers/sub.rs via `use crate::` and `use super::`
    let h_blast = engine.blast_radius(&["crates/app-bin/src/handlers.rs".to_owned()], 2);
    let h_impacted: Vec<&str> = h_blast.impacted_files.iter().map(|i| i.file.as_str()).collect();
    assert!(
        h_impacted.contains(&"crates/app-bin/src/handlers/sub.rs"),
        "expected Edition 2024 submodule sub.rs impacted in {h_impacted:?}"
    );
}

#[test]
fn css_and_scss_import_and_kotlin_gradle_subproject_resolution() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    // CSS / SCSS
    write_repo_file(root, "styles/_tokens.scss", ".btn { color: red; }\n");
    write_repo_file(root, "styles/base.css", ".card { display: block; }\n");
    write_repo_file(
        root,
        "styles/app.scss",
        "@import './tokens';\n@import './base.css';\n",
    );

    // Gradle Kotlin cross-module
    write_repo_file(
        root,
        "settings.gradle.kts",
        "include(\":core-ui\", \":feature:home\")\n",
    );
    write_repo_file(
        root,
        "core-ui/src/main/kotlin/com/demo/ui/Badge.kt",
        "class Badge {}\n",
    );
    write_repo_file(
        root,
        "feature/home/src/main/kotlin/com/demo/home/Screen.kt",
        "import com.demo.ui.Badge\nfun render() {}\n",
    );

    let engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build");

    let scss_blast = engine.blast_radius(&["styles/_tokens.scss".to_owned()], 2);
    assert!(
        scss_blast.impacted_files.iter().any(|i| i.file == "styles/app.scss"),
        "expected styles/app.scss impacted via SCSS partial @import"
    );

    let css_blast = engine.blast_radius(&["styles/base.css".to_owned()], 2);
    assert!(
        css_blast.impacted_files.iter().any(|i| i.file == "styles/app.scss"),
        "expected styles/app.scss impacted via CSS @import"
    );

    let kt_blast =
        engine.blast_radius(&["core-ui/src/main/kotlin/com/demo/ui/Badge.kt".to_owned()], 2);
    assert!(
        kt_blast
            .impacted_files
            .iter()
            .any(|i| i.file == "feature/home/src/main/kotlin/com/demo/home/Screen.kt"),
        "expected Gradle subproject import Screen.kt impacted"
    );
}

#[test]
fn multi_export_barrel_find_origin_matches_requested_symbol() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    write_repo_file(
        root,
        "src/core/auth.ts",
        "export function verifyToken() { return true; }\n",
    );
    write_repo_file(
        root,
        "src/core/user.ts",
        "export function loadUser() { return 'u'; }\n",
    );
    write_repo_file(
        root,
        "src/core/index.ts",
        "export * from './auth';\nexport * from './user';\n",
    );

    let engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build");

    let origin_user = engine
        .find_origin("src/core/index.ts::loadUser")
        .expect("origin for loadUser");
    assert_eq!(origin_user.origin_file, "src/core/user.ts");
    assert_eq!(origin_user.origin_label, "loadUser");

    let origin_auth = engine
        .find_origin("src/core/index.ts::verifyToken")
        .expect("origin for verifyToken");
    assert_eq!(origin_auth.origin_file, "src/core/auth.ts");
    assert_eq!(origin_auth.origin_label, "verifyToken");
}

#[test]
fn load_cached_and_mtime_size_incremental_fast_path() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    write_repo_file(root, "src/a.ts", "export function alpha() { return 1; }\n");
    write_repo_file(root, "src/b.ts", "import { alpha } from './a';\nexport const x = alpha();\n");

    let built = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build");
    assert!(!built.graph.file_stats.is_empty());
    built.save().expect("save graph.bin");

    // load_cached loads cleanly without re-walking or failing
    let mut cached = CodeGraphEngine::load_cached(root, AnvilConfig::default()).expect("load_cached");
    assert_eq!(cached.graph.file_hashes.len(), 2);

    // No-op incremental update uses (mtime, size) fast-path -> 0 modified, 2 unchanged
    let stats_noop = cached.update_incremental().expect("noop incremental");
    assert_eq!(stats_noop.added, 0);
    assert_eq!(stats_noop.modified, 0);
    assert_eq!(stats_noop.removed, 0);
    assert_eq!(stats_noop.unchanged, 2);

    // Corrupt binary logs warning & falls back cleanly to rebuild
    let bin_path = anvil_graph::store::default_bin_path(root);
    fs::write(&bin_path, b"not-valid-postcard").expect("corrupt bin");
    let recovered = CodeGraphEngine::load_cached(root, AnvilConfig::default()).expect("recover");
    assert_eq!(recovered.graph.file_hashes.len(), 2);
}

#[test]
fn merge_skips_common_unresolved_call_names() {
    let front_dir = TempDir::new().expect("front");
    let back_dir = TempDir::new().expect("back");

    write_repo_file(
        front_dir.path(),
        "src/list.ts",
        "export function render(xs: number[]) { return xs.map((x) => x); }\n",
    );
    write_repo_file(
        back_dir.path(),
        "src/util.ts",
        "export function map() { return {}; }\n",
    );

    let front = CodeGraphEngine::build(front_dir.path(), AnvilConfig::default()).expect("front");
    let back = CodeGraphEngine::build(back_dir.path(), AnvilConfig::default()).expect("back");
    let merged = merge_graphs(&[("front", &front.graph), ("back", &back.graph)]);

    let has_spurious_link = merged
        .edges
        .iter()
        .any(|e| e.kind == GraphEdgeKind::SemanticLink);
    assert!(!has_spurious_link, "common method name `map` must not spuriously cross-link");
}

#[test]
fn windows_typecheck_filtering_never_leaks_unrelated_errors() {
    let raw_tsc = "\
src\\unrelated\\broken.ts(12,5): error TS2322: Type 'string' is not assignable to type 'number'.\n\
src/unrelated/other.ts:4:1 - error TS2304: Cannot find name 'Missing'.\n";

    let out = filter_typecheck_output(
        None,
        "tsc --noEmit",
        2,
        raw_tsc,
        &["src/features/clean.ts".to_owned()],
        50,
        1200,
    )
    .expect("filter typecheck");

    assert!(out.passed, "expected pass when 0 errors in impacted files");
    assert_eq!(out.exit_code, 0);
    assert!(out.lines.is_empty(), "must never leak unrelated errors: {:?}", out.lines);
    assert!(
        out.summary.contains("0 errors in modified/impacted files")
            && out.summary.contains("2 unrelated error(s) suppressed"),
        "unexpected summary: {}",
        out.summary
    );

    // Matching Windows backslash path IS retained
    let with_match = "\
src\\unrelated\\broken.ts(12,5): error TS2322: unrelated.\n\
src\\features\\clean.ts(5,3): error TS2345: Argument of type 'number' is not assignable.\n";
    let err_out = filter_typecheck_output(
        None,
        "tsc --noEmit",
        2,
        with_match,
        &["src/features/clean.ts".to_owned()],
        50,
        1200,
    )
    .expect("filter matching");
    assert!(!err_out.passed);
    assert_eq!(err_out.total_errors, 1);
    assert_eq!(err_out.lines.len(), 1);
    assert!(err_out.lines[0].contains("clean.ts"));
}
