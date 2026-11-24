use std::fs;
use std::path::Path;

use anyhow::Result;
use tempfile::TempDir;

use anvil_core::config::{AnvilConfig, StackKind, ToolKind};
use anvil_core::stack_detector::detect_workspace;

fn write_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

#[test]
fn detects_single_repo_react_vite_eslint_sonar() -> Result<()> {
    let tmp = TempDir::new()?;
    write_file(
        &tmp.path().join("package.json"),
        r#"{"dependencies":{"react":"19.0.0"},"devDependencies":{"vite":"6.0.0","vitest":"3.0.0","eslint":"9.0.0"}}"#,
    )?;
    write_file(&tmp.path().join("tsconfig.json"), "{}")?;
    write_file(&tmp.path().join("vite.config.ts"), "")?;
    write_file(&tmp.path().join("sonar-project.properties"), "")?;

    let det = detect_workspace(tmp.path())?;
    assert!(!det.is_multi_repo);
    assert_eq!(det.repos.len(), 1);
    let repo = &det.repos[0];
    assert!(repo.stacks.contains(&StackKind::React));
    assert!(repo.stacks.contains(&StackKind::Vite));
    assert!(repo.stacks.contains(&StackKind::TypeScript));
    assert!(repo.tools.contains(&ToolKind::Vitest));
    assert!(repo.tools.contains(&ToolKind::Eslint));
    assert!(repo.tools.contains(&ToolKind::SonarQube));
    assert!(repo.tools.contains(&ToolKind::Tsc));
    Ok(())
}

#[test]
fn detects_multi_repo_workspace_like_fliiper_centinela() -> Result<()> {
    let tmp = TempDir::new()?;
    let root = tmp.path();

    // Angular + Biome frontend
    write_file(
        &root.join("fliiper-front/package.json"),
        r#"{"dependencies":{"@angular/core":"19.0.0"},"devDependencies":{"jest":"29.0.0"}}"#,
    )?;
    write_file(&root.join("fliiper-front/angular.json"), "{}")?;
    write_file(&root.join("fliiper-front/tsconfig.app.json"), "{}")?;
    write_file(&root.join("fliiper-front/biome.json"), "{}")?;

    // NestJS backend
    write_file(
        &root.join("fliiper-api/package.json"),
        r#"{"dependencies":{"@nestjs/core":"11.0.0"}}"#,
    )?;
    write_file(&root.join("fliiper-api/nest-cli.json"), "{}")?;
    write_file(&root.join("fliiper-api/tsconfig.json"), "{}")?;

    // Python worker
    write_file(
        &root.join("centinela-ml/pyproject.toml"),
        "[tool.ruff]\n[tool.pytest.ini_options]\n",
    )?;

    // Kotlin Android + Roborazzi
    write_file(
        &root.join("fliiper-android/build.gradle.kts"),
        "plugins { id(\"io.github.takahirom.roborazzi\") }\n",
    )?;

    // Flutter mobile
    write_file(&root.join("fliiper-flutter/pubspec.yaml"), "name: fliiper\n")?;

    // Expo RN app
    write_file(
        &root.join("fliiper-expo/package.json"),
        r#"{"dependencies":{"expo":"52.0.0","react-native":"0.76.0"}}"#,
    )?;
    write_file(&root.join("fliiper-expo/app.config.ts"), "")?;

    let det = detect_workspace(root)?;
    assert!(det.is_multi_repo);
    assert_eq!(det.repos.len(), 6);

    let mut cfg = AnvilConfig::default();
    det.apply_to_config(&mut cfg);
    assert_eq!(cfg.subrepos.len(), 6);
    assert!(cfg.stacks.contains(&StackKind::Angular));
    assert!(cfg.stacks.contains(&StackKind::NestJs));
    assert!(cfg.stacks.contains(&StackKind::Python));
    assert!(cfg.stacks.contains(&StackKind::KotlinAndroid));
    assert!(cfg.stacks.contains(&StackKind::FlutterDart));
    assert!(cfg.stacks.contains(&StackKind::ExpoReactNative));
    assert!(cfg.tools.contains(&ToolKind::Biome));
    assert!(cfg.tools.contains(&ToolKind::Ruff));
    assert!(cfg.tools.contains(&ToolKind::Roborazzi));
    assert!(cfg.tools.contains(&ToolKind::DartAnalyze));
    Ok(())
}

#[test]
fn detects_monorepo_subrepos_even_when_root_has_manifest() -> Result<()> {
    let tmp = TempDir::new()?;
    let root = tmp.path();

    write_file(
        &root.join("package.json"),
        r#"{"name":"monorepo","private":true,"devDependencies":{"turbo":"2.0.0","eslint":"9.0.0"}}"#,
    )?;
    write_file(&root.join("tsconfig.base.json"), "{}")?;
    write_file(
        &root.join("apps/web/package.json"),
        r#"{"dependencies":{"react":"19.0.0"}}"#,
    )?;
    write_file(&root.join("apps/web/tsconfig.json"), "{}")?;
    write_file(
        &root.join("services/ml/pyproject.toml"),
        "[tool.pytest.ini_options]\n",
    )?;

    let det = detect_workspace(root)?;
    assert!(det.is_multi_repo);
    assert_eq!(det.repos.len(), 3);
    let paths: Vec<&str> = det.repos.iter().map(|r| r.rel_path.as_str()).collect();
    assert!(paths.contains(&"."));
    assert!(paths.contains(&"apps/web"));
    assert!(paths.contains(&"services/ml"));

    let mut cfg = AnvilConfig::default();
    det.apply_to_config(&mut cfg);
    assert!(cfg.stacks.contains(&StackKind::React));
    assert!(cfg.stacks.contains(&StackKind::Python));
    assert!(cfg.subrepos.iter().any(|s| s.path == "apps/web"));
    assert!(cfg.subrepos.iter().any(|s| s.path == "services/ml"));
    Ok(())
}

#[test]
fn detects_vue_and_svelte_stacks_and_vue_tsc() -> Result<()> {
    let tmp = TempDir::new()?;
    let root = tmp.path();

    write_file(
        &root.join("vue-app/package.json"),
        r#"{"dependencies":{"vue":"3.5.0"},"devDependencies":{"vue-tsc":"2.1.0"}}"#,
    )?;
    write_file(&root.join("vue-app/tsconfig.json"), "{}")?;

    write_file(
        &root.join("svelte-app/package.json"),
        r#"{"dependencies":{"svelte":"5.0.0","@sveltejs/kit":"2.0.0"}}"#,
    )?;

    let det = detect_workspace(root)?;
    assert!(det.is_multi_repo);
    let vue_repo = det
        .repos
        .iter()
        .find(|r| r.rel_path == "vue-app")
        .expect("vue repo");
    assert!(vue_repo.stacks.contains(&StackKind::Vue));
    assert!(vue_repo.tools.contains(&ToolKind::VueTsc));

    let svelte_repo = det
        .repos
        .iter()
        .find(|r| r.rel_path == "svelte-app")
        .expect("svelte repo");
    assert!(svelte_repo.stacks.contains(&StackKind::Svelte));
    Ok(())
}

#[test]
fn detects_godot_stack_tools_and_allowlist() -> Result<()> {
    let tmp = TempDir::new()?;
    let root = tmp.path();

    write_file(&root.join("project.godot"), "config_version=5\n")?;
    write_file(&root.join(".gdlintrc.yaml"), "max-line-length: 100\n")?;
    fs::create_dir_all(root.join("addons/gut"))?;
    fs::create_dir_all(root.join("addons/gdUnit4"))?;

    let det = detect_workspace(root)?;
    let mut cfg = AnvilConfig::default();
    det.apply_to_config(&mut cfg);
    assert!(cfg.stacks.contains(&StackKind::Godot));
    assert!(cfg.tools.contains(&ToolKind::Godot));
    assert!(cfg.tools.contains(&ToolKind::GdLint));
    assert!(cfg.tools.contains(&ToolKind::Gut));
    assert!(cfg.tools.contains(&ToolKind::GdUnit4));
    assert!(cfg.root_allowlist.iter().any(|x| x == "project.godot"));
    Ok(())
}
