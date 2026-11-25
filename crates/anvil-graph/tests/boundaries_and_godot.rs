//! Integration tests for `read_symbol` path containment, Godot resolution/Tier-1, and boundaries.

use std::fs;
use std::path::Path;

use anvil_core::config::{AnvilConfig, BoundaryRule, StackKind};
use anvil_core::traits::{EdgeKind, EdgeRef, Language, SymbolKind};
use anvil_graph::CodeGraphEngine;
use anvil_graph::model::{CodeGraph, GraphNode, NodeKind};
use anvil_graph::resolvers::godot::{GodotProjectContext, resolve_godot_edge};
use tempfile::TempDir;

fn write_file(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(path, content).expect("write fixture file");
}

fn seed_godot_fixture(root: &Path) {
    write_file(
        root,
        "project.godot",
        "[application]\nconfig/name=\"TestGame\"\n\n[autoload]\n\nGameState=\"*res://scripts/game_state.gd\"\n",
    );
    write_file(
        root,
        "scripts/game_state.gd",
        "class_name GameState\nextends Node\n\nfunc get_score() -> int:\n\treturn 10\n",
    );
    write_file(root, "scripts/weapon.gd", "extends Node\n\nfunc fire() -> void:\n\tpass\n");
    write_file(root, "scripts/weapon.gd.uid", "uid://weapon123\n");
    write_file(
        root,
        "scripts/player.gd",
        "extends CharacterBody2D\nconst WeaponScript = preload(\"res://scripts/weapon.gd\")\n\nfunc _ready() -> void:\n\tpass\n",
    );
    write_file(
        root,
        "scenes/player.tscn",
        "[gd_scene load_steps=2 format=3]\n\n[ext_resource type=\"Script\" path=\"res://scripts/player.gd\" id=\"1\"]\n\n[node name=\"Player\" type=\"CharacterBody2D\"]\nscript = ExtResource(\"1\")\n\n[node name=\"Collision\" type=\"CollisionShape2D\" parent=\".\"]\n",
    );
    write_file(
        root,
        "tests/player_test.gd",
        "extends \"res://addons/gut/test.gd\"\n\nfunc test_ok() -> void:\n\tpass\n",
    );
    write_file(root, ".godot/ignored_cache.gd", "extends Node\n");
}

#[test]
fn read_symbol_rejects_paths_outside_repo_root() {
    let outer = TempDir::new().expect("tempdir");
    let outer_root = outer.path();
    write_file(outer_root, "outside.ts", "export function secretFn(): number {\n  return 42;\n}\n");
    let repo_root = outer_root.join("a").join("repo");
    write_file(&repo_root, "src/inside.ts", "export function insideFn(): number {\n  return 1;\n}\n");

    let mut engine = CodeGraphEngine::build(&repo_root, AnvilConfig::default()).expect("build");
    let inside = engine
        .read_symbol("insideFn", false)
        .expect("inside read ok")
        .expect("inside symbol exists");
    assert_eq!(inside.symbol, "insideFn");

    let evil_file = "../../outside.ts";
    engine.graph.insert_node_with_slot(
        GraphNode {
            id: CodeGraph::symbol_node_id(evil_file, "secretFn"),
            kind: NodeKind::Symbol,
            label: "secretFn".to_owned(),
            file: evil_file.to_owned(),
            line: 1,
            end_line: 3,
            language: Some(Language::TypeScript),
            symbol_kind: Some(SymbolKind::Function),
            signature: None,
            exported: true,
            community: None,
        },
        None,
    );
    engine.graph.rebuild_indexes();

    let err = engine
        .read_symbol("secretFn", false)
        .expect_err("must reject path traversal outside repo root");
    assert!(err.to_string().contains("outside repository root"), "unexpected error: {err}");
}

#[test]
fn godot_scene_script_preload_and_blast_radius() {
    let dir = TempDir::new().expect("tempdir");
    seed_godot_fixture(dir.path());

    let cfg = AnvilConfig {
        stacks: vec![StackKind::Godot],
        ..AnvilConfig::default()
    };
    let engine = CodeGraphEngine::build(dir.path(), cfg).expect("build godot graph");
    assert!(!engine.graph.file_hashes.contains_key(".godot/ignored_cache.gd"));

    let report = engine.blast_radius(&["scripts/weapon.gd".to_owned()], 3);
    let impacted: Vec<&str> = report.impacted_files.iter().map(|i| i.file.as_str()).collect();
    assert!(impacted.contains(&"scripts/player.gd"), "got: {impacted:?}");
    assert!(impacted.contains(&"scenes/player.tscn"), "got: {impacted:?}");
}

#[test]
fn godot_autoload_uid_and_extends_resolve_correctly() {
    let dir = TempDir::new().expect("tempdir");
    seed_godot_fixture(dir.path());

    let engine = CodeGraphEngine::build(dir.path(), AnvilConfig::default()).expect("build");
    let ctx = GodotProjectContext::load(dir.path());
    assert_eq!(ctx.autoloads.get("GameState").map(String::as_str), Some("scripts/game_state.gd"));
    assert_eq!(ctx.uid_map.get("uid://weapon123").map(String::as_str), Some("scripts/weapon.gd"));

    let cases = [
        ("GameState", EdgeKind::Calls, "scripts/game_state.gd"),
        ("uid://weapon123", EdgeKind::Imports, "scripts/weapon.gd"),
        ("GameState", EdgeKind::Extends, "scripts/game_state.gd"),
    ];
    for (target, kind, expected_file) in cases {
        let edge = EdgeRef {
            source_file: "scripts/player.gd".to_owned(),
            source_symbol: None,
            target_path_or_symbol: target.to_owned(),
            kind,
            line: 1,
        };
        let nid = resolve_godot_edge(&edge, &engine.graph, &ctx).expect("edge resolves");
        assert_eq!(engine.graph.get_node(nid).map(|n| n.file.as_str()), Some(expected_file));
    }
}

#[test]
fn check_boundaries_catches_forbidden_cross_layer_imports_and_cycles() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write_file(root, "src/domain/order.ts", "import { Button } from '../ui/Button';\nexport function makeOrder() { return Button(); }\n");
    write_file(root, "src/ui/Button.ts", "export function Button() { return 'btn'; }\n");
    write_file(root, "src/cycle/a.ts", "import { b } from './b';\nexport function a() { return b(); }\n");
    write_file(root, "src/cycle/b.ts", "import { a } from './a';\nexport function b() { return a(); }\n");

    let config = AnvilConfig {
        boundaries: vec![BoundaryRule {
            name: "domain-no-ui".to_owned(),
            from_glob: "src/domain/*.ts".to_owned(),
            forbid_glob: "src/ui/*.ts".to_owned(),
            reason: Some("Domain layer must not depend on UI".to_owned()),
        }],
        forbid_import_cycles: true,
        ..AnvilConfig::default()
    };

    let engine = CodeGraphEngine::build(root, config).expect("build graph");
    let diags = engine.check_boundaries(&[]);

    let boundary = diags.iter().find(|d| d.rule == "boundary:domain-no-ui").expect("boundary diag");
    assert_eq!(boundary.file, "src/domain/order.ts");

    let cycle = diags.iter().find(|d| d.rule == "architecture:import-cycle").expect("cycle diag");
    assert!(
        cycle.message.contains("src/cycle/a.ts -> src/cycle/b.ts -> src/cycle/a.ts"),
        "unexpected cycle message: {}",
        cycle.message
    );

    let scoped_clean = engine.check_boundaries(&["src/ui/Button.ts".to_owned()]);
    assert!(scoped_clean.is_empty(), "expected clean scope, got: {scoped_clean:?}");

    write_file(root, "src/domain/barrel.ts", "export { Button } from '../ui/Button';\n");
    let config2 = AnvilConfig {
        boundaries: vec![BoundaryRule {
            name: "domain-no-ui".to_owned(),
            from_glob: "src/domain/*.ts".to_owned(),
            forbid_glob: "src/ui/*.ts".to_owned(),
            reason: None,
        }],
        ..AnvilConfig::default()
    };
    let engine2 = CodeGraphEngine::build(root, config2).expect("rebuild");
    let reexport_diags = engine2.check_boundaries(&["src/domain/barrel.ts".to_owned()]);
    assert!(
        reexport_diags.iter().any(|d| d.rule == "boundary:domain-no-ui"),
        "expected boundary violation on re-export: {reexport_diags:?}"
    );
}
