//! Maven Central (`build.gradle.kts` / `libs.versions.toml`) parser and fetcher.

use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use chrono::DateTime;
use regex::Regex;
use rustc_hash::{FxHashMap, FxHashSet};
use serde_json::Value as JsonValue;
use toml::Value as TomlValue;

use crate::http::{fetch_json_text, percent_encode_component};
use crate::model::{DependencySpec, Ecosystem, PackageHealthInfo};

/// Parses Maven coordinates (`group:artifact`) from Gradle's `libs.versions.toml`.
pub fn parse_libs_versions_toml(
    manifest_path: &str,
    content: &str,
) -> Result<Vec<DependencySpec>> {
    let root: TomlValue = toml::from_str(content)
        .with_context(|| format!("failed to parse TOML in {manifest_path}"))?;

    let versions = collect_version_refs(&root);
    let Some(libraries) = root.get("libraries").and_then(TomlValue::as_table) else {
        return Ok(Vec::new());
    };

    let mut seen = FxHashSet::default();
    let mut specs = Vec::new();
    for entry in libraries.values() {
        if let Some((coord, version)) = extract_catalog_library(entry, &versions)
            && seen.insert(coord.clone())
        {
            specs.push(DependencySpec {
                ecosystem: Ecosystem::Maven,
                name: coord,
                version_req: version,
                manifest_path: manifest_path.to_owned(),
            });
        }
    }

    specs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(specs)
}

/// Parses Maven coordinates (`group:artifact`) from a Kotlin Gradle script (`build.gradle.kts`).
pub fn parse_build_gradle_kts(manifest_path: &str, content: &str) -> Result<Vec<DependencySpec>> {
    static RE_COORD: OnceLock<Regex> = OnceLock::new();
    let re = RE_COORD.get_or_init(|| {
        Regex::new(r#"["']([a-zA-Z0-9_.\-]+:[a-zA-Z0-9_.\-]+)(?::([^"']+))?["']"#)
            .expect("valid maven coordinate regex")
    });

    let mut seen = FxHashSet::default();
    let mut specs = Vec::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("//") || !looks_like_gradle_dep_line(trimmed) {
            continue;
        }
        for caps in re.captures_iter(trimmed) {
            let coord = caps.get(1).map_or("", |m| m.as_str()).to_owned();
            if coord.is_empty() || !seen.insert(coord.clone()) {
                continue;
            }
            let version_req = caps.get(2).map(|m| m.as_str().to_owned());
            specs.push(DependencySpec {
                ecosystem: Ecosystem::Maven,
                name: coord,
                version_req,
                manifest_path: manifest_path.to_owned(),
            });
        }
    }

    specs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(specs)
}

/// Fetches package metadata from Maven Central Solr search API for `"group:artifact"`.
pub fn fetch_maven(package: &str) -> Result<PackageHealthInfo> {
    let Some((group, artifact)) = package.split_once(':') else {
        bail!("invalid Maven coordinate `{package}`, expected `group:artifact`");
    };
    let g_enc = percent_encode_component(group);
    let a_enc = percent_encode_component(artifact);
    let url = format!(
        "https://search.maven.org/solrsearch/select?q=g:%22{g_enc}%22+AND+a:%22{a_enc}%22&rows=1&wt=json"
    );
    let body = fetch_json_text(&url)?;
    parse_maven_search_json(package, &body)
}

/// Parses a Maven Central Solr JSON response into [`PackageHealthInfo`].
pub fn parse_maven_search_json(package: &str, body: &str) -> Result<PackageHealthInfo> {
    let root: JsonValue = serde_json::from_str(body)
        .with_context(|| format!("invalid Maven Central JSON for `{package}`"))?;
    let doc = root
        .get("response")
        .and_then(|r| r.get("docs"))
        .and_then(JsonValue::as_array)
        .and_then(|docs| docs.first());

    let latest_version = doc
        .and_then(|d| d.get("latestVersion"))
        .and_then(JsonValue::as_str)
        .map(str::to_owned);

    let last_publish = doc
        .and_then(|d| d.get("timestamp"))
        .and_then(JsonValue::as_i64)
        .and_then(DateTime::from_timestamp_millis);

    Ok(PackageHealthInfo {
        ecosystem: Ecosystem::Maven,
        name: package.to_owned(),
        latest_version,
        last_publish,
        deprecated: false,
        deprecation_message: None,
        archived: false,
        repository_url: Some("https://central.sonatype.com/".to_owned()),
    })
}

fn collect_version_refs(root: &TomlValue) -> FxHashMap<String, String> {
    let mut versions = FxHashMap::default();
    if let Some(table) = root.get("versions").and_then(TomlValue::as_table) {
        for (k, v) in table {
            if let Some(s) = v.as_str() {
                versions.insert(k.clone(), s.to_owned());
            }
        }
    }
    versions
}

fn extract_catalog_library(
    entry: &TomlValue,
    versions: &FxHashMap<String, String>,
) -> Option<(String, Option<String>)> {
    if let Some(shorthand) = entry.as_str() {
        let mut parts = shorthand.splitn(3, ':');
        let group = parts.next()?;
        let artifact = parts.next()?;
        let ver = parts.next().map(str::to_owned);
        return Some((format!("{group}:{artifact}"), ver));
    }

    let table = entry.as_table()?;
    let coord = if let Some(module) = table.get("module").and_then(TomlValue::as_str) {
        module.to_owned()
    } else {
        let group = table.get("group").and_then(TomlValue::as_str)?;
        let name = table.get("name").and_then(TomlValue::as_str)?;
        format!("{group}:{name}")
    };

    let version = match table.get("version") {
        Some(TomlValue::String(s)) => Some(s.clone()),
        Some(TomlValue::Table(vt)) => vt
            .get("ref")
            .and_then(TomlValue::as_str)
            .and_then(|r| versions.get(r).cloned()),
        _ => None,
    };
    Some((coord, version))
}

fn looks_like_gradle_dep_line(trimmed: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "implementation",
        "api",
        "testImplementation",
        "androidTestImplementation",
        "compileOnly",
        "runtimeOnly",
        "ksp",
        "kapt",
        "annotationProcessor",
        "debugImplementation",
        "releaseImplementation",
    ];
    PREFIXES.iter().any(|p| trimmed.contains(p))
}
