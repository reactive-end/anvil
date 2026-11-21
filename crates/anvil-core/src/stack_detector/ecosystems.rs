//! Per-ecosystem manifest and tool detection helpers.

use std::fs;
use std::path::Path;

use anyhow::Result;
use rustc_hash::FxHashSet;

use crate::config::{StackKind, ToolKind};

pub(super) fn detect_js_ts_stacks(
    dir: &Path,
    stacks: &mut FxHashSet<StackKind>,
    tools: &mut FxHashSet<ToolKind>,
) -> Result<()> {
    let pkg_path = dir.join("package.json");
    let pkg_text = if pkg_path.exists() {
        fs::read_to_string(&pkg_path).unwrap_or_default()
    } else {
        String::new()
    };

    if has_any_file(dir, &["tsconfig.json", "tsconfig.app.json", "tsconfig.base.json"]) {
        stacks.insert(StackKind::TypeScript);
        tools.insert(ToolKind::Tsc);
    }
    if dir.join("angular.json").exists() || pkg_text.contains("\"@angular/core\"") {
        stacks.insert(StackKind::Angular);
    }
    if dir.join("nest-cli.json").exists() || pkg_text.contains("\"@nestjs/core\"") {
        stacks.insert(StackKind::NestJs);
    }
    if has_any_prefix(dir, &["next.config."]) || pkg_text.contains("\"next\"") {
        stacks.insert(StackKind::NextJs);
    }
    if has_any_prefix(dir, &["astro.config."]) || pkg_text.contains("\"astro\"") {
        stacks.insert(StackKind::Astro);
    }
    if has_any_prefix(dir, &["vite.config."]) || pkg_text.contains("\"vite\"") {
        stacks.insert(StackKind::Vite);
    }
    if pkg_text.contains("\"react\"") && !pkg_text.contains("\"react-native\"") {
        stacks.insert(StackKind::React);
    }
    if pkg_text.contains("\"expo\"") || pkg_text.contains("\"react-native\"") || is_expo_app(dir) {
        stacks.insert(StackKind::ExpoReactNative);
    }
    detect_js_tools(dir, &pkg_text, tools);
    Ok(())
}

fn detect_js_tools(dir: &Path, pkg_text: &str, tools: &mut FxHashSet<ToolKind>) {
    if has_any_prefix(dir, &[".eslintrc", "eslint.config."]) || pkg_text.contains("\"eslint\"") {
        tools.insert(ToolKind::Eslint);
    }
    if has_any_file(dir, &["biome.json", "biome.jsonc"]) || pkg_text.contains("\"@biomejs/biome\"") {
        tools.insert(ToolKind::Biome);
    }
    if has_any_prefix(dir, &["vitest.config."]) || pkg_text.contains("\"vitest\"") {
        tools.insert(ToolKind::Vitest);
    }
    if has_any_prefix(dir, &["jest.config."]) || pkg_text.contains("\"jest\"") {
        tools.insert(ToolKind::Jest);
    }
}

pub(super) fn detect_python_stack(
    dir: &Path,
    stacks: &mut FxHashSet<StackKind>,
    tools: &mut FxHashSet<ToolKind>,
) -> Result<()> {
    let pyproject = dir.join("pyproject.toml");
    let has_py = pyproject.exists()
        || dir.join("requirements.txt").exists()
        || dir.join("setup.py").exists();
    if !has_py {
        return Ok(());
    }
    stacks.insert(StackKind::Python);
    let content = fs::read_to_string(&pyproject).unwrap_or_default();
    if has_any_file(dir, &["ruff.toml", ".ruff.toml"]) || content.contains("[tool.ruff]") {
        tools.insert(ToolKind::Ruff);
    }
    if content.contains("pytest") || dir.join("pytest.ini").exists() {
        tools.insert(ToolKind::Pytest);
    }
    if content.contains("pyright") || dir.join("pyrightconfig.json").exists() {
        tools.insert(ToolKind::Pyright);
    }
    if content.contains("mypy") || dir.join("mypy.ini").exists() {
        tools.insert(ToolKind::Mypy);
    }
    Ok(())
}

pub(super) fn detect_kotlin_android_stack(
    dir: &Path,
    stacks: &mut FxHashSet<StackKind>,
    tools: &mut FxHashSet<ToolKind>,
) -> Result<()> {
    let has_gradle = has_any_file(
        dir,
        &["build.gradle.kts", "build.gradle", "settings.gradle.kts", "settings.gradle"],
    );
    if !has_gradle {
        return Ok(());
    }
    stacks.insert(StackKind::KotlinAndroid);
    let gradle_texts = [
        dir.join("build.gradle.kts"),
        dir.join("app/build.gradle.kts"),
        dir.join("gradle/libs.versions.toml"),
    ]
    .into_iter()
    .filter_map(|p| fs::read_to_string(p).ok())
    .collect::<Vec<_>>()
    .join("\n");

    if gradle_texts.contains("roborazzi") {
        tools.insert(ToolKind::Roborazzi);
    }
    if gradle_texts.contains("ktlint") {
        tools.insert(ToolKind::Ktlint);
    }
    if gradle_texts.contains("detekt") {
        tools.insert(ToolKind::Detekt);
    }
    Ok(())
}

pub(super) fn detect_flutter_rust_tools(
    dir: &Path,
    stacks: &mut FxHashSet<StackKind>,
    tools: &mut FxHashSet<ToolKind>,
) {
    if dir.join("pubspec.yaml").exists() {
        stacks.insert(StackKind::FlutterDart);
        tools.insert(ToolKind::DartAnalyze);
    }
    if dir.join("Cargo.toml").exists() {
        stacks.insert(StackKind::Rust);
    }
    if dir.join("sonar-project.properties").exists() {
        tools.insert(ToolKind::SonarQube);
    }
}

fn is_expo_app(dir: &Path) -> bool {
    if has_any_file(dir, &["app.config.ts", "app.config.js"]) {
        return true;
    }
    fs::read_to_string(dir.join("app.json"))
        .map(|s| s.contains("\"expo\""))
        .unwrap_or(false)
}

fn has_any_file(dir: &Path, names: &[&str]) -> bool {
    names.iter().any(|name| dir.join(name).exists())
}

fn has_any_prefix(dir: &Path, prefixes: &[&str]) -> bool {
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let s = name.to_string_lossy();
        prefixes.iter().any(|p| s.starts_with(p))
    })
}
