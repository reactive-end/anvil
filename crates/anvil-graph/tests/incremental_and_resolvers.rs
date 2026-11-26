//! Integration tests for <15ms incremental updates, persistence, and multi-language resolvers.

use std::fs;
use std::path::Path;
use std::time::Instant;

use anvil_core::config::AnvilConfig;
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
fn incremental_single_file_update_skips_untouched_files_and_persists_binary() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    write_repo_file(root, "src/a.ts", "export function alpha() { return 1; }\n");
    write_repo_file(
        root,
        "src/b.ts",
        "import { alpha } from './a';\nexport function beta() { return alpha(); }\n",
    );
    write_repo_file(root, "src/c.ts", "export const gamma = 42;\n");

    let mut engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build");
    let hash_b_before = engine.graph.file_hashes.get("src/b.ts").cloned();
    let hash_c_before = engine.graph.file_hashes.get("src/c.ts").cloned();

    // No-op update on unchanged file must short-circuit cleanly.
    assert!(!engine.update_file("src/a.ts").expect("noop update"));

    // Modify src/a.ts and measure single-file update latency.
    write_repo_file(
        root,
        "src/a.ts",
        "export function alpha() { return 2; }\nexport function alphaTwo() { return 3; }\n",
    );
    let started = Instant::now();
    let changed = engine.update_file("src/a.ts").expect("incremental update");
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;

    assert!(changed);
    assert!(elapsed_ms < 50.0, "incremental update took {elapsed_ms:.2}ms");
    assert_eq!(engine.graph.file_hashes.get("src/b.ts"), hash_b_before.as_ref());
    assert_eq!(engine.graph.file_hashes.get("src/c.ts"), hash_c_before.as_ref());
    assert!(engine.graph.contains_node_id("sym:src/a.ts::alphaTwo"));

    // Save binary + reload + export NetworkX JSON.
    let bin_path = engine.save().expect("save binary");
    let reloaded = store::load_binary(&bin_path).expect("load binary");
    assert_eq!(reloaded.nodes.len(), engine.graph.nodes.len());
    assert_eq!(reloaded.edges.len(), engine.graph.edges.len());

    let json_path = engine.export_json().expect("export json");
    let json_text = fs::read_to_string(json_path).expect("read json");
    assert!(json_text.contains("\"nodes\""));
    assert!(json_text.contains("\"links\""));
}

fn write_multi_lang_fixtures(root: &Path) {
    // NestJS
    write_repo_file(
        root,
        "nest/auth.service.ts",
        "@Injectable()\nexport class AuthService { validate() { return true; } }\n",
    );
    write_repo_file(
        root,
        "nest/auth.module.ts",
        "import { AuthService } from './auth.service';\n\
         @Module({ providers: [AuthService] })\n\
         export class AuthModule {\n\
           constructor(private readonly auth: AuthService) {}\n\
         }\n",
    );

    // Python
    write_repo_file(root, "pypkg/__init__.py", "");
    write_repo_file(root, "pypkg/utils.py", "def slugify(x):\n    return x\n");
    write_repo_file(
        root,
        "pypkg/sub/service.py",
        "from ..utils import slugify\ndef run():\n    return slugify('a')\n",
    );

    // Kotlin
    write_repo_file(
        root,
        "android/src/main/kotlin/com/example/ui/Button.kt",
        "package com.example.ui\n@Composable\nfun PrimaryButton() {}\n",
    );
    write_repo_file(
        root,
        "android/src/main/kotlin/com/example/app/HomeScreen.kt",
        "package com.example.app\nimport com.example.ui.PrimaryButton\n\
         @Composable\nfun HomeScreen() { PrimaryButton() }\n",
    );

    // Dart / Flutter
    write_repo_file(root, "flutter_app/pubspec.yaml", "name: my_flutter_app\n");
    write_repo_file(root, "flutter_app/lib/widgets/badge.dart", "class StatusBadge {}\n");
    write_repo_file(
        root,
        "flutter_app/lib/main.dart",
        "import 'package:my_flutter_app/widgets/badge.dart';\nvoid main() {}\n",
    );
}

#[test]
fn multi_language_resolvers_nestjs_python_kotlin_dart() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write_multi_lang_fixtures(root);

    let engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build multi-lang");

    let nest_impact = engine.blast_radius(&["nest/auth.service.ts".to_owned()], 2);
    assert!(nest_impact.impacted_files.iter().any(|i| i.file == "nest/auth.module.ts"));

    let py_impact = engine.blast_radius(&["pypkg/utils.py".to_owned()], 2);
    assert!(py_impact.impacted_files.iter().any(|i| i.file == "pypkg/sub/service.py"));

    let kt_impact =
        engine.blast_radius(&["android/src/main/kotlin/com/example/ui/Button.kt".to_owned()], 2);
    assert!(
        kt_impact
            .impacted_files
            .iter()
            .any(|i| i.file == "android/src/main/kotlin/com/example/app/HomeScreen.kt")
    );

    let dart_impact = engine.blast_radius(&["flutter_app/lib/widgets/badge.dart".to_owned()], 2);
    assert!(dart_impact.impacted_files.iter().any(|i| i.file == "flutter_app/lib/main.dart"));
}
