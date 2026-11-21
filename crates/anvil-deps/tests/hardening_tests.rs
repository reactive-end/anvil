//! Hardening Wave H2 tests: abbreviated npm, Cargo, requirements.txt, build.gradle, Google Maven, and bounded parallel fetch.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration as StdDuration, Instant};

use anvil_deps::cargo_toml::{parse_cargo_toml, parse_crates_io_json};
use anvil_deps::maven::{
    is_google_maven_group, parse_build_gradle, parse_google_maven_metadata_xml,
};
use anvil_deps::npm::{parse_npm_registry_json, parse_package_json};
use anvil_deps::pypi::parse_requirements_txt;
use anvil_deps::{
    AuditOptions, DepIssueKind, Ecosystem, PackageHealthInfo, RegistryFetcher, audit_manifests,
    is_supported_manifest, parse_manifest,
};
use anyhow::Result;
use chrono::{Datelike, Duration, Timelike, Utc};
use tempfile::TempDir;

struct SlowBoundedFetcher {
    in_flight: AtomicUsize,
    max_in_flight: AtomicUsize,
    calls: AtomicUsize,
}

impl SlowBoundedFetcher {
    fn new() -> Self {
        Self {
            in_flight: AtomicUsize::new(0),
            max_in_flight: AtomicUsize::new(0),
            calls: AtomicUsize::new(0),
        }
    }
}

impl RegistryFetcher for SlowBoundedFetcher {
    fn fetch(&self, ecosystem: Ecosystem, package: &str) -> Result<PackageHealthInfo> {
        let cur = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_in_flight.fetch_max(cur, Ordering::SeqCst);
        self.calls.fetch_add(1, Ordering::Relaxed);
        thread::sleep(StdDuration::from_millis(40));
        self.in_flight.fetch_sub(1, Ordering::SeqCst);

        Ok(PackageHealthInfo {
            ecosystem,
            name: package.to_owned(),
            latest_version: Some("1.0.0".to_owned()),
            last_publish: Some(Utc::now() - Duration::days(10)),
            deprecated: package == "pkg-19",
            deprecation_message: (package == "pkg-19").then(|| "deprecated test".to_owned()),
            archived: false,
            repository_url: None,
        })
    }
}

#[test]
fn parses_abbreviated_npm_install_v1_and_latest_shapes() {
    let abbr = r#"{
      "name": "left-pad",
      "modified": "2024-06-15T08:30:00.000Z",
      "dist-tags": { "latest": "1.3.0" },
      "versions": {
        "1.3.0": { "name": "left-pad", "version": "1.3.0", "deprecated": "use String.prototype.padStart" }
      }
    }"#;
    let info = parse_npm_registry_json("left-pad", abbr).expect("abbreviated npm");
    assert_eq!(info.latest_version.as_deref(), Some("1.3.0"));
    assert!(info.deprecated);
    assert_eq!(
        info.deprecation_message.as_deref(),
        Some("use String.prototype.padStart")
    );
    assert!(info.last_publish.is_some());

    let latest_only = r#"{
      "name": "lodash",
      "version": "4.17.21",
      "repository": { "type": "git", "url": "https://github.com/lodash/lodash.git" }
    }"#;
    let latest_info = parse_npm_registry_json("lodash", latest_only).expect("/latest npm");
    assert_eq!(latest_info.latest_version.as_deref(), Some("4.17.21"));
    assert!(!latest_info.deprecated);
    assert_eq!(
        latest_info.repository_url.as_deref(),
        Some("https://github.com/lodash/lodash.git")
    );
}

#[test]
fn parses_cargo_toml_and_crates_io_payload() {
    let manifest = "[package]\nname = \"demo\"\nversion = \"0.1.0\"\n\
[dependencies]\nanyhow = \"1.0\"\nmy_serde = { package = \"serde\", version = \"1.0\" }\n\
local_core = { path = \"../core\" }\nws_dep = { workspace = true }\n\
[dev-dependencies]\ntempfile = \"3\"\n[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n";
    let specs = parse_cargo_toml("Cargo.toml", manifest).expect("cargo toml");
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["anyhow", "libc", "serde", "tempfile"]);
    assert!(specs.iter().all(|s| s.ecosystem == Ecosystem::Cargo));

    let payload = r#"{
      "crate": {
        "id": "serde",
        "updated_at": "2024-09-01T12:00:00Z",
        "max_stable_version": "1.0.210",
        "repository": "https://github.com/serde-rs/serde"
      },
      "versions": [
        { "num": "1.0.210", "yanked": true, "created_at": "2024-09-01T12:00:00Z" }
      ]
    }"#;
    let info = parse_crates_io_json("serde", payload).expect("crates.io json");
    assert_eq!(info.ecosystem, Ecosystem::Cargo);
    assert_eq!(info.latest_version.as_deref(), Some("1.0.210"));
    assert!(info.deprecated);
    assert!(info.last_publish.is_some());
    assert_eq!(
        info.repository_url.as_deref(),
        Some("https://github.com/serde-rs/serde")
    );
}

#[test]
fn parses_requirements_txt_and_groovy_build_gradle() {
    let reqs = r#"
# Base requirements
-r base.txt
--index-url https://pypi.org/simple
-e ./local-pkg
git+https://github.com/org/repo.git
fastapi>=0.115.0 # web framework
pydantic[email]~=2.9 ; python_version >= '3.10'
requests==2.32.3 \
    --hash=sha256:deadbeef
"#;
    let py_specs = parse_requirements_txt("requirements.txt", reqs).expect("requirements.txt");
    let py_names: Vec<&str> = py_specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(py_names, vec!["fastapi", "pydantic", "requests"]);

    let groovy = r#"
dependencies {
    implementation 'com.squareup.okhttp3:okhttp:4.12.0'
    implementation platform('androidx.compose:compose-bom:2024.09.00')
    implementation 'androidx.core:core-ktx'
    testImplementation "junit:junit:4.13.2"
}
"#;
    let gradle_specs = parse_build_gradle("app/build.gradle", groovy).expect("build.gradle");
    let gradle_names: Vec<&str> = gradle_specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        gradle_names,
        vec![
            "androidx.compose:compose-bom",
            "androidx.core:core-ktx",
            "com.squareup.okhttp3:okhttp",
            "junit:junit"
        ]
    );
}

#[test]
fn google_maven_group_detection_and_metadata_xml_parsing() {
    assert!(is_google_maven_group("androidx.core"));
    assert!(is_google_maven_group("com.google.android.material"));
    assert!(is_google_maven_group("com.android.tools.build"));
    assert!(!is_google_maven_group("com.squareup.okhttp3"));

    let xml = r#"<?xml version='1.0' encoding='UTF-8'?>
<metadata>
  <groupId>androidx.core</groupId>
  <artifactId>core-ktx</artifactId>
  <versioning>
    <latest>1.15.0</latest>
    <release>1.15.0</release>
    <versions><version>1.14.0</version><version>1.15.0</version></versions>
    <lastUpdated>20241030175232</lastUpdated>
  </versioning>
</metadata>"#;
    let info = parse_google_maven_metadata_xml("androidx.core:core-ktx", xml).expect("google xml");
    assert_eq!(info.ecosystem, Ecosystem::Maven);
    assert_eq!(info.latest_version.as_deref(), Some("1.15.0"));
    let ts = info.last_publish.expect("lastUpdated parsed");
    assert_eq!((ts.year(), ts.month(), ts.day()), (2024, 10, 30));
    assert_eq!((ts.hour(), ts.minute(), ts.second()), (17, 52, 32));
}

#[test]
fn bounded_parallel_audit_caps_concurrency_at_8_and_speeds_up_20_deps() {
    let tmp = TempDir::new().expect("tempdir");
    let mut deps_json = String::from("{ \"dependencies\": {");
    for i in 0..20 {
        if i > 0 {
            deps_json.push(',');
        }
        deps_json.push_str(&format!(" \"pkg-{i:02}\": \"^1.0.0\""));
    }
    deps_json.push_str(" } }");
    fs::write(tmp.path().join("package.json"), deps_json).expect("write package.json");

    // Verify new manifest filenames are also recognized by audit helpers.
    for name in ["Cargo.toml", "requirements.txt", "build.gradle"] {
        assert!(is_supported_manifest(&PathBuf::from(name)));
    }
    fs::write(tmp.path().join("requirements.txt"), "requests==2.32.3\n").expect("write reqs");
    assert_eq!(
        parse_manifest(tmp.path(), &PathBuf::from("requirements.txt"))
            .expect("parse reqs")
            .len(),
        1
    );

    let fetcher = SlowBoundedFetcher::new();
    let opts = AuditOptions::default();
    let start = Instant::now();
    let report = audit_manifests(tmp.path(), &[PathBuf::from("package.json")], &fetcher, &opts)
        .expect("parallel audit");
    let elapsed = start.elapsed();

    assert_eq!(report.checked_packages, 20);
    assert_eq!(report.network_fetches, 20);
    assert_eq!(fetcher.calls.load(Ordering::Relaxed), 20);
    let max_parallel = fetcher.max_in_flight.load(Ordering::SeqCst);
    assert!(
        (2..=8).contains(&max_parallel),
        "expected bounded concurrency in 2..=8, got {max_parallel}"
    );
    assert!(
        elapsed.as_millis() < 600,
        "expected parallel speedup (<600ms vs 800ms serial), took {}ms",
        elapsed.as_millis()
    );
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].package, "pkg-19");
    assert_eq!(report.findings[0].issues, vec![DepIssueKind::Deprecated]);
}

#[test]
fn skips_non_registry_npm_specifiers_in_monorepos() {
    let manifest = r#"{
      "name": "monorepo-web",
      "dependencies": {
        "@acme/ui": "workspace:*",
        "local-pkg": "file:../pkg",
        "linked-lib": "link:../x",
        "portaled": "portal:../portal",
        "patched": "patch:lodash@npm%3A4.17.21#./lodash.patch",
        "git-https": "git+https://github.com/org/repo.git#main",
        "git-proto": "git://github.com/org/repo.git",
        "http-tarball": "http://example.com/pkg.tgz",
        "https-tarball": "https://example.com/pkg.tgz",
        "gh-shorthand": "github:org/repo",
        "react": "^19.0.0"
      },
      "devDependencies": {
        "@acme/eslint-config": "workspace:^",
        "vitest": "^2.0.0"
      }
    }"#;

    let specs = parse_package_json("apps/web/package.json", manifest).expect("package.json");
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["react", "vitest"]);
}

#[test]
fn supports_and_parses_variant_requirements_txt_manifests() {
    for name in [
        "requirements.txt",
        "requirements-dev.txt",
        "requirements_prod.txt",
        "deploy/requirements-ci.txt",
    ] {
        assert!(
            is_supported_manifest(&PathBuf::from(name)),
            "expected {name} to be supported"
        );
    }
    assert!(!is_supported_manifest(&PathBuf::from("notes.txt")));

    let tmp = TempDir::new().expect("tempdir");
    fs::write(
        tmp.path().join("requirements-dev.txt"),
        "pytest>=8.0.0\nruff==0.6.0\n",
    )
    .expect("write requirements-dev.txt");

    let specs =
        parse_manifest(tmp.path(), &PathBuf::from("requirements-dev.txt")).expect("parse dev reqs");
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, vec!["pytest", "ruff"]);
    assert!(specs.iter().all(|s| s.ecosystem == Ecosystem::Pypi));
}

