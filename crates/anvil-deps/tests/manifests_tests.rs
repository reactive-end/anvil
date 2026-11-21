//! Manifest and registry-payload parsing tests for all 4 ecosystems.

use anvil_deps::maven::{
    parse_build_gradle_kts, parse_libs_versions_toml, parse_maven_search_json,
};
use anvil_deps::npm::{parse_npm_registry_json, parse_package_json};
use anvil_deps::pubdev::{parse_pubdev_registry_json, parse_pubspec_yaml};
use anvil_deps::pypi::{parse_pypi_registry_json, parse_pyproject_toml};
use anvil_deps::Ecosystem;

#[test]
fn parses_npm_package_json_and_registry_payload() {
    let manifest = r#"{
      "name": "web-app",
      "dependencies": { "react": "^19.0.0", "@scope/pkg": "1.2.3" },
      "devDependencies": { "tslint": "^6.1.3" }
    }"#;
    let specs = parse_package_json("package.json", manifest).expect("npm manifest");
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["@scope/pkg", "react", "tslint"]);
    assert!(specs.iter().all(|s| s.ecosystem == Ecosystem::Npm));

    let registry = r#"{
      "dist-tags": { "latest": "6.1.3" },
      "time": { "6.1.3": "2020-12-01T12:00:00.000Z" },
      "versions": { "6.1.3": { "deprecated": "TSLint has been deprecated" } }
    }"#;
    let info = parse_npm_registry_json("tslint", registry).expect("npm registry");
    assert!(info.deprecated);
    assert_eq!(info.latest_version.as_deref(), Some("6.1.3"));
    assert_eq!(
        info.deprecation_message.as_deref(),
        Some("TSLint has been deprecated")
    );
}

#[test]
fn parses_pypi_pyproject_toml_and_registry_payload() {
    let manifest = r#"
[project]
name = "svc"
dependencies = [
  "fastapi>=0.115.0",
  "pydantic[email]~=2.9 ; python_version >= '3.10'"
]
[project.optional-dependencies]
dev = ["ruff>=0.6.0"]

[tool.poetry.dependencies]
python = "^3.12"
requests = "^2.32.0"
"#;
    let specs = parse_pyproject_toml("pyproject.toml", manifest).expect("pyproject");
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["fastapi", "pydantic", "requests", "ruff"]);

    let registry = r#"{
      "info": {
        "version": "1.0.0",
        "yanked": true,
        "classifiers": ["Development Status :: 7 - Inactive"]
      },
      "releases": {
        "1.0.0": [{ "upload_time_iso_8601": "2021-01-10T00:00:00Z" }]
      }
    }"#;
    let info = parse_pypi_registry_json("oldpkg", registry).expect("pypi registry");
    assert!(info.deprecated);
    assert!(info.archived);
    assert!(info.last_publish.is_some());
}

#[test]
fn parses_pubspec_yaml_and_pubdev_registry_payload() {
    let manifest = r#"
name: mobile_app
dependencies:
  flutter:
    sdk: flutter
  http: ^1.2.0
  local_core:
    path: ../local_core
dev_dependencies:
  flutter_lints: ^4.0.0 # inline comment
"#;
    let specs = parse_pubspec_yaml("pubspec.yaml", manifest).expect("pubspec");
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["flutter_lints", "http"]);

    let registry = r#"{
      "isDiscontinued": true,
      "replacedBy": "dio",
      "latest": {
        "version": "0.13.6",
        "published": "2023-05-01T10:00:00Z",
        "pubspec": { "repository": "https://github.com/example/pkg" }
      }
    }"#;
    let info = parse_pubdev_registry_json("old_http", registry).expect("pubdev registry");
    assert!(info.deprecated);
    assert!(info.archived);
    assert_eq!(
        info.deprecation_message.as_deref(),
        Some("discontinued; replaced by dio")
    );
}

#[test]
fn parses_maven_gradle_and_version_catalog_and_solr_payload() {
    let gradle = r#"
dependencies {
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    implementation(platform("androidx.compose:compose-bom:2024.09.00"))
    testImplementation("junit:junit:4.13.2")
}
"#;
    let gradle_specs = parse_build_gradle_kts("app/build.gradle.kts", gradle).expect("gradle");
    let gradle_names: Vec<&str> = gradle_specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        gradle_names,
        vec![
            "androidx.compose:compose-bom",
            "com.squareup.okhttp3:okhttp",
            "junit:junit"
        ]
    );

    let catalog = r#"
[versions]
retrofit = "2.11.0"

[libraries]
retrofit-core = { module = "com.squareup.retrofit2:retrofit", version.ref = "retrofit" }
gson = { group = "com.google.code.gson", name = "gson", version = "2.11.0" }
"#;
    let cat_specs = parse_libs_versions_toml("gradle/libs.versions.toml", catalog).expect("toml");
    let cat_names: Vec<&str> = cat_specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        cat_names,
        vec![
            "com.google.code.gson:gson",
            "com.squareup.retrofit2:retrofit"
        ]
    );

    let solr = r#"{
      "response": {
        "docs": [{ "latestVersion": "4.12.0", "timestamp": 1700000000000 }]
      }
    }"#;
    let info =
        parse_maven_search_json("com.squareup.okhttp3:okhttp", solr).expect("maven registry");
    assert_eq!(info.latest_version.as_deref(), Some("4.12.0"));
    assert!(info.last_publish.is_some());
}
