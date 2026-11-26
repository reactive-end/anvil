//! Integration tests for pure-Rust Godot 4 (`.gd`, `.tscn`, `.tres`, `.gdshader`) analysis.

use std::path::Path;

use anvil_ast::{analyze_file, detect_language};
use anvil_core::config::AnvilConfig;
use anvil_core::traits::{EdgeKind, FileAstFacts, Language, Severity, SymbolKind};

fn cfg() -> AnvilConfig {
    AnvilConfig::default()
}

fn has_diag(facts: &FileAstFacts, rule: &str, line: u32) -> bool {
    facts.diagnostics.iter().any(|d| d.rule == rule && d.line == line)
}

fn find_sym<'a>(facts: &'a FileAstFacts, name: &str) -> &'a anvil_core::traits::SymbolDef {
    facts
        .symbols
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("missing symbol `{name}` in {:?}", facts.symbols))
}

fn has_edge(facts: &FileAstFacts, kind: EdgeKind, target: &str, line: u32) -> bool {
    facts
        .edges
        .iter()
        .any(|e| e.kind == kind && e.target_path_or_symbol == target && e.line == line)
}

#[test]
fn detects_godot_languages_and_analyzes_gdshader_cleanly() {
    assert_eq!(detect_language(Path::new("scripts/player.gd")), Language::GdScript);
    assert_eq!(detect_language(Path::new("scenes/player.tscn")), Language::GodotScene);
    assert_eq!(detect_language(Path::new("data/stats.tres")), Language::GodotScene);
    assert_eq!(detect_language(Path::new("shaders/glow.gdshader")), Language::GdShader);

    let shader = "shader_type spatial;\nvoid fragment() {\n\tALBEDO = vec3(1.0);\n}\n";
    let facts = analyze_file(Path::new("shaders/glow.gdshader"), shader, &cfg()).expect("gdshader");
    assert_eq!(facts.language, Language::GdShader);
    assert!(facts.diagnostics.is_empty(), "unexpected: {:?}", facts.diagnostics);
}

#[test]
fn gdscript_extracts_symbols_with_block_spans_and_edges() {
    let src = "\
class_name PlayerController
extends CharacterBody3D

signal health_changed(new_hp: int)
@export var move_speed: float = 6.5

const BULLET_SCENE := preload(\"res://scenes/bullet.tscn\")
var helper: Script = load(\"res://scripts/helper.gd\")

func _ready() -> void:
\tvar target: Node = get_node(\"Target\")
\thealth_changed.connect(_on_health_changed)

func _on_health_changed(new_hp: int) -> void:
\tprint(new_hp)
";
    let facts = analyze_file(Path::new("scripts/player.gd"), src, &cfg()).expect("gdscript");
    assert_eq!(facts.language, Language::GdScript);
    assert!(facts.diagnostics.is_empty(), "unexpected: {:?}", facts.diagnostics);

    let cls = find_sym(&facts, "PlayerController");
    assert_eq!(cls.kind, SymbolKind::Class);
    assert!(cls.exported);
    assert_eq!(find_sym(&facts, "health_changed").kind, SymbolKind::Variable);
    assert_eq!(find_sym(&facts, "move_speed").kind, SymbolKind::Variable);

    let ready_fn = find_sym(&facts, "_ready");
    assert_eq!(ready_fn.kind, SymbolKind::Function);
    assert_eq!((ready_fn.line, ready_fn.end_line), (10, 12));

    let handler_fn = find_sym(&facts, "_on_health_changed");
    assert_eq!((handler_fn.line, handler_fn.end_line), (14, 15));

    assert!(has_edge(&facts, EdgeKind::Extends, "CharacterBody3D", 2));
    assert!(has_edge(&facts, EdgeKind::Imports, "res://scenes/bullet.tscn", 7));
    assert!(has_edge(&facts, EdgeKind::Imports, "res://scripts/helper.gd", 8));
    assert!(facts.edges.iter().any(|e| {
        e.kind == EdgeKind::Calls
            && e.target_path_or_symbol == "_on_health_changed"
            && e.source_symbol.as_deref() == Some("_ready")
            && e.line == 12
    }));
}

#[test]
fn gdscript_extracts_inner_methods_and_quoted_extends() {
    let src = "\
extends \"res://scripts/base_entity.gd\"

class InnerHelper:
\tfunc compute(x: int) -> int:
\t\treturn x * 2
";
    let facts = analyze_file(Path::new("scripts/sub.gd"), src, &cfg()).expect("sub gdscript");
    assert!(facts.diagnostics.is_empty(), "unexpected: {:?}", facts.diagnostics);
    assert!(has_edge(&facts, EdgeKind::Extends, "res://scripts/base_entity.gd", 1));

    let method = find_sym(&facts, "compute");
    assert_eq!(method.kind, SymbolKind::Method);
    assert_eq!(method.qualified_name, "InnerHelper.compute");
    assert_eq!((method.line, method.end_line), (4, 5));
}

#[test]
fn gdscript_flags_godot3_syntax_without_false_positives_on_godot4_names() {
    let src = "\
tool
extends Spatial
onready var mesh: MeshInstance = $Mesh
export var speed: float = 5.0
export(int, 0, 10) var hp: int = 10
var mat: SpatialMaterial = SpatialMaterial.new()
var body2d: KinematicBody2D = null
var body3d: KinematicBody = null
var sprite: AnimatedSprite = null
var p2: Position2D = null
var p3: Position3D = null

func _ready() -> void:
\tvar win: Vector2i = OS.window_size
\tvar rad: float = deg2rad(90.0)
\tvar deg: float = rad2deg(rad)
\tvar r: float = rand_range(0.0, 1.0)
\tvar bullet: Node = preload(\"res://b.tscn\").instance()
\tmove_and_slide(Vector3.UP)
\tyield(get_tree(), \"timeout\")
";
    let facts = analyze_file(Path::new("scripts/legacy.gd"), src, &cfg()).expect("legacy");
    let gd3_lines: Vec<u32> = facts
        .diagnostics
        .iter()
        .filter(|d| d.rule == "godot4-no-godot3-syntax")
        .map(|d| d.line)
        .collect();
    for expected in [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 14, 15, 16, 17, 18, 19, 20] {
        assert!(
            gd3_lines.contains(&expected),
            "missing godot3 diag at line {expected}: {gd3_lines:?}"
        );
    }

    let valid_gd4 = "\
@tool
extends CharacterBody3D
@onready var mesh: MeshInstance3D = $Mesh
@export var speed: float = 5.0
var root_node: Node3D = null
var mat: StandardMaterial3D = StandardMaterial3D.new()
var sprite: AnimatedSprite2D = null
var marker2: Marker2D = null
var marker3: Marker3D = null
var label_txt: String = \"Spatial MeshInstance KinematicBody yield() onready var\"

# Spatial KinematicBody2D move_and_slide(vel)
func _physics_process(_delta: float) -> void:
\tmove_and_slide()
\tawait get_tree().process_frame
";
    let clean = analyze_file(Path::new("scripts/modern.gd"), valid_gd4, &cfg()).expect("modern");
    assert!(clean.diagnostics.is_empty(), "unexpected diagnostics: {:?}", clean.diagnostics);
}

#[test]
fn gdscript_flags_typing_tabs_velocity_and_todo_rules() {
    let src = "\
extends CharacterBody3D

var velocity: Vector3 = Vector3.ZERO
var untyped = 10
var inferred_null := null
var typed_ok: int = 1
var inferred_ok := 2

func bad_sig(
\ta,
\tb: int
):
\t# TODO: refactor movement
    var spaced_ok: int = 3
\tvar note: String = \"\"\"
var untyped_in_multiline_str = 1
\t\"\"\"
";
    let facts = analyze_file(Path::new("scripts/bad_rules.gd"), src, &cfg()).expect("bad rules");
    assert!(has_diag(&facts, "godot4-characterbody-velocity", 3));
    assert!(has_diag(&facts, "godot4-static-typing", 4));
    assert!(has_diag(&facts, "godot4-static-typing", 5));
    assert!(!has_diag(&facts, "godot4-static-typing", 6));
    assert!(!has_diag(&facts, "godot4-static-typing", 7));
    assert!(has_diag(&facts, "godot4-static-typing", 9)); // missing -> ReturnType
    assert!(has_diag(&facts, "godot4-static-typing", 10)); // untyped param `a`
    assert!(!has_diag(&facts, "godot4-static-typing", 11)); // typed param `b: int`
    assert!(has_diag(&facts, "no-todo-comments", 13));
    assert!(has_diag(&facts, "godot4-tabs-only", 14));
    assert_eq!(
        facts.diagnostics.iter().find(|d| d.rule == "godot4-tabs-only").map(|d| d.severity),
        Some(Severity::Warning)
    );
    assert!(!has_diag(&facts, "godot4-static-typing", 16));
}

#[test]
fn gdscript_supports_anvil_allow_on_same_or_previous_line() {
    let src = "\
extends CharacterBody2D

# anvil-allow(godot4-characterbody-velocity): custom override wrapper
var velocity: Vector2 = Vector2.ZERO
var dynamic_cfg = {} # anvil-allow(godot4-static-typing): dynamic JSON payload
# anvil-allow(no-todo-comments): tracked in GD-101
# TODO: replace with typed resource
# anvil-allow(godot4-tabs-only): aligned data block
    var aligned: int = 1
# anvil-allow(godot4-no-godot3-syntax): legacy compat shim
var old_mat: SpatialMaterial = null
";
    let facts = analyze_file(Path::new("scripts/allowed.gd"), src, &cfg()).expect("allowed");
    assert!(facts.diagnostics.is_empty(), "unexpected diagnostics: {:?}", facts.diagnostics);
}

#[test]
fn tscn_extracts_ext_resources_nodes_and_checks_collision_children() {
    let bad_scene = "\
[gd_scene load_steps=3 format=3]

[ext_resource type=\"Script\" path=\"res://scripts/player.gd\" id=\"1_scr\"]
[ext_resource type=\"PackedScene\" path=\"res://scenes/gun.tscn\" id=\"2_gun\"]

[node name=\"Player\" type=\"CharacterBody3D\"]
script = ExtResource(\"1_scr\")

[node name=\"Mesh\" type=\"MeshInstance3D\" parent=\".\"]

[node name=\"Gun\" parent=\".\" instance=ExtResource(\"2_gun\")]

[node name=\"HitBox\" type=\"Area3D\" parent=\"Mesh\"]
";
    let bad = analyze_file(Path::new("scenes/player.tscn"), bad_scene, &cfg()).expect("bad tscn");
    assert!(bad.edges.iter().any(|e| {
        e.kind == EdgeKind::Imports && e.target_path_or_symbol == "res://scripts/player.gd"
    }));
    assert!(bad.edges.iter().any(|e| {
        e.kind == EdgeKind::RendersTemplate && e.target_path_or_symbol == "res://scenes/gun.tscn"
    }));

    let player_sym = bad.symbols.iter().find(|s| s.name == "Player").expect("Player node");
    assert_eq!(player_sym.kind, SymbolKind::Component);
    assert!(player_sym.signature.as_deref().unwrap_or("").contains("res://scripts/player.gd"));

    assert!(has_diag(&bad, "godot4-collision-required", 6));
    assert!(has_diag(&bad, "godot4-collision-required", 13));
    assert!(!has_diag(&bad, "godot4-collision-required", 11)); // instanced prefab skipped

    let good_scene = "\
[gd_scene load_steps=2 format=3]

[node name=\"Player\" type=\"CharacterBody3D\"]

[node name=\"BodyShape\" type=\"CollisionShape3D\" parent=\".\"]

[node name=\"Mesh\" type=\"MeshInstance3D\" parent=\".\"]

[node name=\"HitBox\" type=\"Area3D\" parent=\"Mesh\"]

[node name=\"HitShape\" type=\"CollisionShape3D\" parent=\"Mesh/HitBox\"]
";
    let good = analyze_file(Path::new("scenes/good.tscn"), good_scene, &cfg()).expect("good tscn");
    assert!(good.diagnostics.is_empty(), "unexpected diagnostics: {:?}", good.diagnostics);
}
