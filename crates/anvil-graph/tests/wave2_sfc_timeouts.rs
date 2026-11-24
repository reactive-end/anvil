//! Wave 2 regression tests for SFC collection (`.vue`/`.svelte`/`.astro`) and `vue-tsc` selection.

use std::fs;
use std::path::Path;

use anvil_core::config::{AnvilConfig, StackKind};
use anvil_graph::CodeGraphEngine;
use anvil_graph::builder::collect_source_files;
use anvil_graph::impact::build_vue_or_ts_typecheck_command;
use tempfile::TempDir;

fn write_repo_file(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(path, content).expect("write fixture file");
}

fn create_mock_bin(work_dir: &Path, name: &str) {
    let bin_dir = work_dir.join("node_modules").join(".bin");
    fs::create_dir_all(&bin_dir).expect("create .bin");
    if cfg!(windows) {
        let p = bin_dir.join(format!("{name}.cmd"));
        fs::write(&p, "@echo off\r\nexit /b 0\r\n").expect("write mock .cmd");
    } else {
        let p = bin_dir.join(name);
        fs::write(&p, "#!/bin/sh\nexit 0\n").expect("write mock bin");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&p).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&p, perms).expect("chmod");
        }
    }
}

#[test]
fn collect_source_files_includes_sfc_and_ignores_build_dirs() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    write_repo_file(root, "src/App.vue", "<template><div/></template>\n");
    write_repo_file(root, "src/Counter.svelte", "<script>let n = 0;</script>\n");
    write_repo_file(root, "src/pages/index.astro", "---\n---\n<html/>\n");
    write_repo_file(root, "src/main.ts", "export const ok = true;\n");

    // Ignored build output directories
    write_repo_file(root, ".svelte-kit/generated/root.svelte", "<slot/>\n");
    write_repo_file(root, ".astro/types.d.ts", "export {};\n");
    write_repo_file(root, ".astro/generated.astro", "<div/>\n");

    let collected = collect_source_files(root);
    let rels: Vec<&str> = collected.iter().map(|(_, rel)| rel.as_str()).collect();

    assert!(rels.contains(&"src/App.vue"), "expected .vue in {rels:?}");
    assert!(rels.contains(&"src/Counter.svelte"), "expected .svelte in {rels:?}");
    assert!(rels.contains(&"src/pages/index.astro"), "expected .astro in {rels:?}");
    assert!(rels.contains(&"src/main.ts"), "expected .ts in {rels:?}");

    assert!(
        !rels.iter().any(|r| r.starts_with(".svelte-kit/")),
        ".svelte-kit must be ignored: {rels:?}"
    );
    assert!(
        !rels.iter().any(|r| r.starts_with(".astro/")),
        ".astro must be ignored: {rels:?}"
    );
}

#[test]
fn vue_tsc_selected_for_vue_package_json_and_mock_bin() {
    let log_dir = TempDir::new().expect("log_dir");
    let work_dir = TempDir::new().expect("work_dir");
    let root = work_dir.path();

    write_repo_file(
        root,
        "package.json",
        r#"{ "name": "vue-app", "dependencies": { "vue": "^3.4.0" } }"#,
    );
    write_repo_file(root, "src/App.vue", "<template><div/></template>\n");
    create_mock_bin(root, "vue-tsc");
    create_mock_bin(root, "tsc");

    let (bin, args, tool) =
        build_vue_or_ts_typecheck_command(log_dir.path(), root, &[]).expect("vue-tsc command");
    assert_eq!(tool, "vue-tsc");
    assert!(bin.to_string_lossy().to_ascii_lowercase().contains("vue-tsc"));
    assert_eq!(args, vec!["--noEmit".to_owned()]);

    // End-to-end StackKind::Vue routes through run_ts_typecheck using vue-tsc
    let cfg = AnvilConfig {
        stacks: vec![StackKind::Vue],
        ..Default::default()
    };
    let engine = CodeGraphEngine::build(root, cfg).expect("build vue graph");
    let outputs = engine
        .run_impacted_typecheck(&["src/App.vue".to_owned()])
        .expect("run vue typecheck");
    assert_eq!(outputs.len(), 1);
    assert!(outputs[0].passed, "expected vue-tsc pass: {:?}", outputs[0]);
    assert_eq!(outputs[0].exit_code, 0);
}
