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
