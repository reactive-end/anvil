use std::fs;
use std::path::Path;

use anvil_core::check_cache::{
    CachedTier1, compute_scope_key, compute_scope_key_with_subrepos, lookup, store,
};
use anvil_core::diff_grep::{compress_diff, compressed_grep, is_noise_file};
use anvil_core::stats::{StatsEvent, estimate_tokens, load_stats, record_event, reset_stats};
use anvil_core::traits::{EdgeKind, SymbolDef, SymbolKind};
use chrono::Utc;
use tempfile::TempDir;

fn write_file(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dirs");
    }
    fs::write(path, bytes).expect("write test file");
}

#[test]
fn check_cache_hits_and_invalidates_on_edit() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    write_file(root, "src/lib.rs", b"pub fn a() -> i32 { 1 }\n");
    write_file(root, "src/util.rs", b"pub fn b() -> i32 { 2 }\n");
    write_file(root, "Cargo.toml", b"[package]\nname = \"demo\"\n");

    let k1 =
        compute_scope_key(root, &["src/util.rs".to_owned(), "./src/lib.rs".to_owned()], "tier1-v1")
            .expect("compute key");
    let k1_reordered =
        compute_scope_key(root, &["src\\lib.rs".to_owned(), "src/util.rs".to_owned()], "tier1-v1")
            .expect("compute reordered key");
    assert_eq!(k1, k1_reordered);
    assert!(lookup(root, &k1).expect("lookup miss").is_none());

    store(
        root,
        CachedTier1 {
            key: k1.clone(),
            passed: true,
            summary: "PASS tier1".to_owned(),
            recorded_at: Utc::now(),
        },
    )
    .expect("store cache entry");
    let hit = lookup(root, &k1).expect("lookup hit").expect("cached entry");
    assert!(hit.passed && hit.summary == "PASS tier1");

    write_file(root, "src/lib.rs", b"pub fn a() -> i32 { 42 }\n");
    let k2 =
        compute_scope_key(root, &["src/lib.rs".to_owned(), "src/util.rs".to_owned()], "tier1-v1")
            .expect("compute key after edit");
    assert_ne!(k1, k2);
    assert!(lookup(root, &k2).expect("lookup after edit").is_none());

    write_file(root, "package.json", b"{\"name\":\"demo\"}\n");
    let k3 =
        compute_scope_key(root, &["src/lib.rs".to_owned(), "src/util.rs".to_owned()], "tier1-v1")
            .expect("compute key after manifest edit");
    assert_ne!(k2, k3);
}

#[test]
fn check_cache_with_subrepos_invalidates_on_subrepo_manifests_and_rules() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    let files = vec!["apps/web/src/App.vue".to_owned()];
    let subrepos = vec![".".to_owned(), "apps/web".to_owned(), "services/api/".to_owned()];

    let base = compute_scope_key_with_subrepos(root, &files, &subrepos, "salt")
        .expect("compute base subrepo key");

    write_file(root, "apps/web/tsconfig.app.json", b"{\"strict\":true}\n");
    let after_ts = compute_scope_key_with_subrepos(root, &files, &subrepos, "salt")
        .expect("key after tsconfig.app.json");
    assert_ne!(base, after_ts);

    write_file(root, "services/api/phpstan.neon", b"parameters:\n  level: 8\n");
    let after_phpstan = compute_scope_key_with_subrepos(root, &files, &subrepos, "salt")
        .expect("key after phpstan.neon");
    assert_ne!(after_ts, after_phpstan);

    write_file(root, ".harness/rules/r1.json", b"{\"id\":\"r1\"}\n");
    let after_rule_add = compute_scope_key_with_subrepos(root, &files, &subrepos, "salt")
        .expect("key after rule add");
    assert_ne!(after_phpstan, after_rule_add);

    write_file(root, ".harness/rules/r1.json", b"{\"id\":\"r1\",\"v\":2}\n");
    let after_rule_edit = compute_scope_key_with_subrepos(root, &files, &subrepos, "salt")
        .expect("key after rule edit");
    assert_ne!(after_rule_add, after_rule_edit);
}

#[test]
fn check_cache_evicts_entries_past_64() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();

    for idx in 0..70 {
        store(
            root,
            CachedTier1 {
                key: format!("key-{idx}"),
                passed: true,
                summary: format!("entry {idx}"),
                recorded_at: Utc::now(),
            },
        )
        .expect("store bounded entry");
    }
    assert!(lookup(root, "key-5").expect("evicted key-5").is_none());
    assert!(lookup(root, "key-69").expect("kept key-69").is_some());
}

#[test]
fn stats_record_accumulate_and_reset() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();
    assert_eq!((estimate_tokens(0), estimate_tokens(4), estimate_tokens(5)), (0, 1, 2));
    assert_eq!(load_stats(root).expect("default stats").hook_calls, 0);

    let events = [
        StatsEvent::HookCall { elapsed_ms: 14 },
        StatsEvent::HookCall { elapsed_ms: 6 },
        StatsEvent::CheckCacheHit,
        StatsEvent::CompressorSaved { raw_bytes: 1000, compact_bytes: 200 },
        StatsEvent::SymbolReadSaved { tokens_saved: 120 },
        StatsEvent::OutlineSaved { tokens_saved: 80 },
        StatsEvent::TokenGuard { blocked: true, tokens_avoided: 40 },
        StatsEvent::TokenGuard { blocked: false, tokens_avoided: 0 },
        StatsEvent::RuleHit { rule: "no-ts-ignore".to_owned() },
        StatsEvent::RuleHit { rule: "no-ts-ignore".to_owned() },
    ];
    for event in events {
        record_event(root, event).expect("record event");
    }

    let loaded = load_stats(root).expect("load accumulated stats");
    assert_eq!(
        (
            loaded.hook_calls,
            loaded.total_hook_ms,
            loaded.check_cache_hits,
            loaded.tokens_saved_compressor,
            loaded.tokens_saved_symbol_read,
            loaded.tokens_saved_outline,
            loaded.token_guard_blocks,
            loaded.token_guard_warnings,
            loaded.rule_hits.get("no-ts-ignore").copied(),
        ),
        (2, 20, 1, 200, 120, 80, 1, 1, Some(2))
    );

    reset_stats(root).expect("reset stats");
    assert_eq!(load_stats(root).expect("load reset"), Default::default());
}

#[test]
fn is_noise_file_matches_lockfiles_minified_and_generated_dirs() {
    let noisy = [
        "package-lock.json",
        "Cargo.lock",
        "pnpm-lock.yaml",
        "yarn.lock",
        "poetry.lock",
        "pubspec.lock",
        "web/assets/app.min.js",
        "styles/theme.min.css",
        "dist/bundle.js.map",
        "packages/ui/build/index.js",
        "node_modules/pkg/index.js",
        "target/debug/anvil",
        ".next/server/app.js",
        ".harness/.state/graph.bin",
        "graph.json",
        "graph.bin",
    ];
    for path in noisy {
        assert!(is_noise_file(path), "expected noisy: {path}");
    }
    for clean in ["src/build.rs", "src/app.ts", "crates/anvil-core/src/lib.rs"] {
        assert!(!is_noise_file(clean), "expected clean: {clean}");
    }
}

#[test]
fn compress_diff_skips_noise_files_and_enforces_token_cap() {
    let mut raw_diff = String::from(
        "diff --git a/package-lock.json b/package-lock.json\n\
         index 1111111..2222222 100644\n\
         --- a/package-lock.json\n+++ b/package-lock.json\n@@ -1,3 +1,30 @@\n",
    );
    for idx in 0..30 {
        raw_diff.push_str(&format!("+  \"dep-{idx}\": \"1.0.{idx}\",\n"));
    }
    raw_diff.push_str(
        "diff --git a/src/auth.ts b/src/auth.ts\n\
         similarity index 92%\n\
         index 3333333..4444444 100644\n\
         --- a/src/auth.ts\n+++ b/src/auth.ts\n@@ -10,2 +10,3 @@ export function login() {\n\
         +  return validateToken();\n }\n",
    );

    let full = compress_diff(&raw_diff, 100, 1000);
    assert_eq!(full.skipped_files, vec!["package-lock.json"]);
    assert!(!full.truncated && full.estimated_tokens_saved > 0);
    assert!(full.text.contains("skipped 1 noisy file(s): package-lock.json"));
    assert!(full.text.contains("return validateToken();"));
    assert!(!full.text.contains("dep-0") && !full.text.contains("similarity index"));

    let capped = compress_diff(&raw_diff, 100, 20);
    assert!(capped.truncated && estimate_tokens(capped.text.len()) <= 20);
}

#[test]
fn compressed_grep_skips_noise_binaries_and_oversized_files() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();

    write_file(
        root,
        "src/service.ts",
        b"  export function findUserToken() {\n    return 'findUserToken';\n  }\n",
    );
    write_file(root, "package-lock.json", b"\"name\": \"findUserToken\"\n");
    write_file(root, "dist/app.min.js", b"function findUserToken(){return 1}\n");
    write_file(root, "node_modules/dep/index.js", b"exports.findUserToken = 1;\n");
    write_file(root, ".git/config", b"findUserToken = true\n");
    write_file(root, ".harness/.state/cache.txt", b"findUserToken\n");
    write_file(root, "assets/logo.bin", b"findUserToken\x00binarypayload");

    let mut huge = vec![b'a'; 520 * 1024];
    huge.extend_from_slice(b"\nfindUserToken\n");
    write_file(root, "src/huge.txt", &huge);

    let res = compressed_grep(root, "findUserToken", 10, 500).expect("grep ok");
    assert_eq!((res.files_searched, res.total_matches, res.truncated), (1, 2, false));
    assert_eq!(
        res.matches,
        vec![
            "src/service.ts:1: export function findUserToken() {",
            "src/service.ts:2: return 'findUserToken';",
        ]
    );

    let limited = compressed_grep(root, "findUserToken", 1, 500).expect("grep max_matches");
    assert!(limited.truncated && limited.matches.len() == 1 && limited.total_matches == 2);

    write_file(root, ".gitignore", b"ignored_out/\n*.log\n");
    write_file(root, "ignored_out/gen.ts", b"const findUserToken = 1;\n");
    write_file(root, "debug.log", b"findUserToken\n");
    write_file(root, ".harness/.worktrees/wt1/sub.ts", b"const findUserToken = 2;\n");
    let clean = compressed_grep(root, "findUserToken", 10, 500).expect("clean gitignore/worktree");
    assert_eq!(clean.matches, res.matches, "ignored dirs, logs, and worktrees must not match");
    assert_eq!(clean.files_searched, 1, "only src/service.ts is searched; dot-files stay hidden");

    let long_match = format!("const findUserToken = \"{}\";\n", "x".repeat(400));
    write_file(root, "src/long.ts", long_match.as_bytes());
    let capped = compressed_grep(root, "findUserToken", 10, 20).expect("capped long match");
    assert!(capped.truncated && !capped.matches.is_empty());
    assert!(capped.matches.iter().any(|m| m.ends_with("...") && m.len() <= 80));
}

#[test]
fn traits_symbol_def_and_route_edges_serde_compat() {
    let legacy_json = r#"{
        "name": "AuthService",
        "qualified_name": "AuthService",
        "kind": "class",
        "file": "src/auth.ts",
        "line": 12,
        "signature": null,
        "exported": true
    }"#;
    let parsed: SymbolDef =
        serde_json::from_str(legacy_json).expect("deserialize legacy SymbolDef");
    assert_eq!((parsed.kind, parsed.line, parsed.end_line), (SymbolKind::Class, 12, 0));

    let exposes = serde_json::to_string(&EdgeKind::ExposesRoute).expect("serialize ExposesRoute");
    let calls = serde_json::to_string(&EdgeKind::CallsRoute).expect("serialize CallsRoute");
    assert_eq!((exposes.as_str(), calls.as_str()), ("\"exposes_route\"", "\"calls_route\""));
}

#[test]
fn godot_parser_log_rotation_and_tier0_cache_work() {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();

    let raw = "Godot Engine v4.3.stable\nSCRIPT ERROR: Parse Error: Identifier not found\n   at: GDScript::reload (res://player.gd:12)\n";
    let compressed = anvil_core::compressor::compress_output(
        Some(root),
        "godot --headless --check-only",
        1,
        raw,
        50,
        1200,
    )
    .expect("compress godot output");
    assert!(!compressed.passed);
    assert_eq!(compressed.tool, "godot");
    assert!(compressed.lines.iter().any(|l| l.contains("SCRIPT ERROR:")));

    for i in 0..54 {
        let _ = anvil_core::compressor::compress_output(
            Some(root),
            "godot",
            1,
            &format!("ERROR: {i}"),
            50,
            1200,
        );
    }
    let log_count = fs::read_dir(root.join(".harness/.state/logs")).expect("logs dir").count();
    assert_eq!(log_count, 50, "raw logs should rotate at max 50");

    write_file(root, "src/ok.ts", b"export const a: number = 1;\n");
    let cfg = anvil_core::config::AnvilConfig::default();
    let rules_hash = anvil_core::tier0_cache::compute_rules_hash(root, &cfg);
    let mut cache = anvil_core::tier0_cache::Tier0Cache::load(root);
    assert!(!cache.is_clean_hit(&root.join("src/ok.ts"), "src/ok.ts", &rules_hash));
    cache.record_clean(&root.join("src/ok.ts"), "src/ok.ts", &rules_hash);
    cache.save(root).expect("save tier0 cache");

    let reloaded = anvil_core::tier0_cache::Tier0Cache::load(root);
    assert!(reloaded.is_clean_hit(&root.join("src/ok.ts"), "src/ok.ts", &rules_hash));
    assert!(!reloaded.is_clean_hit(&root.join("src/ok.ts"), "src/ok.ts", "different-hash"));
    assert!(is_noise_file(".godot/imported/icon.ctex"));
    assert!(is_noise_file("assets/hero.png.import"));
    assert!(is_noise_file("Node_Modules/pkg/index.js"));
    assert!(is_noise_file("CARGO.LOCK"));

    let diff_spaces = concat!(
        "diff --git \"a/src/diseño final.ts\" \"b/src/diseño final.ts\"\n",
        "index 1111111..2222222 100644\n",
        "--- \"a/src/diseño final.ts\"\n",
        "+++ \"b/src/diseño final.ts\"\n",
        "@@ -1 +1 @@\n+export const x = 1;\n",
        "diff --git \"a/CARGO.LOCK\" \"b/CARGO.LOCK\"\n",
        "+dep\n",
    );
    let comp = compress_diff(diff_spaces, 50, 500);
    assert_eq!(comp.skipped_files, vec!["CARGO.LOCK"]);
    assert!(comp.text.contains("export const x = 1;"));
}
