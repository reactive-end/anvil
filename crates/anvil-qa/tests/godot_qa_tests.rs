use std::path::Path;

use anvil_core::config::{AnvilConfig, StackKind};
use anvil_qa::QaEngine;
use anvil_qa::godot::{audit_godot_scene_tree, build_godot_movie_command};
use anvil_qa::model::{QaViolationKind, UiPlatform};
use anvil_qa::onboarding::detect_ui_platform;

#[test]
fn build_godot_movie_command_uses_write_movie_without_headless_and_clamps_frames() {
    let cmd = build_godot_movie_command(
        Path::new("godot4"),
        Path::new("game"),
        Path::new("out/frame.png"),
        0,
    );
    assert_eq!(cmd.program, "godot4");
    assert_eq!(
        cmd.args,
        vec![
            "--path",
            "game",
            "--write-movie",
            "out/frame.png",
            "--fixed-fps",
            "30",
            "--quit-after",
            "1",
        ]
    );
    assert!(!cmd.args.iter().any(|a| a == "--headless"));

    let multi = build_godot_movie_command(
        Path::new("/usr/bin/godot"),
        Path::new("/proj"),
        Path::new("/tmp/shot.png"),
        12,
    );
    assert!(multi.args.windows(2).any(|w| w == ["--quit-after", "12"]));
}

#[test]
fn audit_godot_scene_tree_flags_missing_camera_lighting_and_collision_shape() {
    let broken_3d = r#"
[gd_scene format=3]

[node name="World" type="Node3D"]

[node name="Player" type="CharacterBody3D" parent="."]

[node name="Mesh" type="MeshInstance3D" parent="Player"]
"#;

    let violations = audit_godot_scene_tree(broken_3d, "scenes/world.tscn");
    let kinds: Vec<QaViolationKind> = violations.iter().map(|v| v.kind).collect();

    assert_eq!(violations.len(), 3, "expected 3 violations: {violations:?}");
    assert!(kinds.contains(&QaViolationKind::Godot3dMissingCamera));
    assert!(kinds.contains(&QaViolationKind::Godot3dMissingLighting));
    assert!(kinds.contains(&QaViolationKind::GodotMissingCollisionShape));
    assert_eq!(QaViolationKind::Godot3dMissingCamera.as_str(), "godot-3d-missing-camera");
    assert_eq!(QaViolationKind::Godot3dMissingLighting.as_str(), "godot-3d-missing-lighting");
    assert_eq!(
        QaViolationKind::GodotMissingCollisionShape.as_str(),
        "godot-missing-collision-shape"
    );
}

#[test]
fn audit_godot_scene_tree_passes_well_formed_3d_scene_and_qa_engine_delegates() {
    let clean_3d = r#"
[gd_scene format=3]

[node name="World" type="Node3D"]

[node name="Camera3D" type="Camera3D" parent="."]

[node name="Sun" type="DirectionalLight3D" parent="."]

[node name="Floor" type="StaticBody3D" parent="."]

[node name="FloorShape" type="CollisionShape3D" parent="Floor"]

[node name="Player" type="CharacterBody3D" parent="."]

[node name="PlayerHull" type="CollisionPolygon3D" parent="Player"]
"#;

    let violations = audit_godot_scene_tree(clean_3d, "scenes/level.tscn");
    assert!(violations.is_empty(), "expected 0 violations, got {violations:?}");

    let engine = QaEngine::new(".");
    assert!(engine.audit_godot_scene(clean_3d, "scenes/level.tscn").is_empty());

    let godot_cfg = AnvilConfig { stacks: vec![StackKind::Godot], ..Default::default() };
    assert_eq!(detect_ui_platform(&godot_cfg), UiPlatform::None);
}

#[test]
fn audit_godot_scene_tree_checks_2d_physics_bodies_without_requiring_3d_camera_or_lighting() {
    let scene_2d = r#"
[gd_scene format=3]

[node name="Level2D" type="Node2D"]

[node name="Hero" type="CharacterBody2D" parent="."]

[node name="Enemy" type="Area2D" parent="."]

[node name="EnemyShape" type="CollisionShape2D" parent="Enemy"]
"#;

    let violations = audit_godot_scene_tree(scene_2d, "scenes/level_2d.tscn");
    assert_eq!(violations.len(), 1, "expected 1 violation: {violations:?}");
    assert_eq!(violations[0].kind, QaViolationKind::GodotMissingCollisionShape);
    assert!(violations[0].selector.contains("Hero"));
}
