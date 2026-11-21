//! PyPI (`pyproject.toml` + `pypi.org/pypi/<pkg>/json`) dependency parser and fetcher.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rustc_hash::FxHashSet;
use serde_json::Value as JsonValue;
use toml::Value as TomlValue;

use crate::http::{fetch_json_text, parse_rfc3339_utc, percent_encode_component};
use crate::model::{DependencySpec, Ecosystem, PackageHealthInfo};

/// Parses PEP 621 and Poetry dependency declarations from `pyproject.toml`.
pub fn parse_pyproject_toml(manifest_path: &str, content: &str) -> Result<Vec<DependencySpec>> {
    let root: TomlValue = toml::from_str(content)
        .with_context(|| format!("failed to parse TOML in {manifest_path}"))?;

    let mut seen = FxHashSet::default();
    let mut specs = Vec::new();

    if let Some(project) = root.get("project").and_then(TomlValue::as_table) {
        collect_pep621_deps(project, manifest_path, &mut seen, &mut specs);
    }

    if let Some(poetry) = root
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(TomlValue::as_table)
    {
        collect_poetry_deps(poetry, manifest_path, &mut seen, &mut specs);
    }

    specs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(specs)
}

/// Fetches package metadata from `https://pypi.org/pypi/<package>/json`.
pub fn fetch_pypi(package: &str) -> Result<PackageHealthInfo> {
    let encoded = percent_encode_component(package);
    let url = format!("https://pypi.org/pypi/{encoded}/json");
    let body = fetch_json_text(&url)?;
    parse_pypi_registry_json(package, &body)
}

/// Parses a PyPI JSON API response into [`PackageHealthInfo`].
pub fn parse_pypi_registry_json(package: &str, body: &str) -> Result<PackageHealthInfo> {
    let root: JsonValue = serde_json::from_str(body)
        .with_context(|| format!("invalid PyPI JSON for `{package}`"))?;
    let info = root.get("info");

    let latest_version = info
        .and_then(|i| i.get("version"))
        .and_then(JsonValue::as_str)
        .map(str::to_owned);

    let yanked = info
        .and_then(|i| i.get("yanked"))
        .and_then(JsonValue::as_bool)
        .unwrap_or(false);

    let classifiers_inactive = info
        .and_then(|i| i.get("classifiers"))
        .and_then(JsonValue::as_array)
        .is_some_and(|arr| {
            arr.iter()
                .filter_map(JsonValue::as_str)
                .any(|c| c.contains("7 - Inactive"))
        });

    let last_publish = latest_version
        .as_deref()
        .and_then(|ver| latest_release_timestamp(&root, ver));

    let repository_url = info
        .and_then(|i| i.get("home_page").or_else(|| i.get("package_url")))
        .and_then(JsonValue::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    Ok(PackageHealthInfo {
        ecosystem: Ecosystem::Pypi,
        name: package.to_owned(),
        latest_version,
        last_publish,
        deprecated: yanked,
        deprecation_message: yanked.then(|| "latest PyPI release is yanked".to_owned()),
        archived: classifiers_inactive,
        repository_url,
    })
}

fn collect_pep621_deps(
    project: &toml::map::Map<String, TomlValue>,
    manifest_path: &str,
    seen: &mut FxHashSet<String>,
    specs: &mut Vec<DependencySpec>,
) {
    if let Some(deps) = project.get("dependencies").and_then(TomlValue::as_array) {
        for item in deps.iter().filter_map(TomlValue::as_str) {
            push_pep508(item, manifest_path, seen, specs);
        }
    }
    if let Some(opt) = project
        .get("optional-dependencies")
        .and_then(TomlValue::as_table)
    {
        for group in opt.values().filter_map(TomlValue::as_array) {
            for item in group.iter().filter_map(TomlValue::as_str) {
                push_pep508(item, manifest_path, seen, specs);
            }
        }
    }
}

fn collect_poetry_deps(
    poetry: &toml::map::Map<String, TomlValue>,
    manifest_path: &str,
    seen: &mut FxHashSet<String>,
    specs: &mut Vec<DependencySpec>,
) {
    for key in ["dependencies", "dev-dependencies"] {
        if let Some(table) = poetry.get(key).and_then(TomlValue::as_table) {
            push_poetry_table(table, manifest_path, seen, specs);
        }
    }
    if let Some(groups) = poetry.get("group").and_then(TomlValue::as_table) {
        for group in groups.values().filter_map(TomlValue::as_table) {
            if let Some(table) = group.get("dependencies").and_then(TomlValue::as_table) {
                push_poetry_table(table, manifest_path, seen, specs);
            }
        }
    }
}

fn push_poetry_table(
    table: &toml::map::Map<String, TomlValue>,
    manifest_path: &str,
    seen: &mut FxHashSet<String>,
    specs: &mut Vec<DependencySpec>,
) {
    for (name, val) in table {
        if name.eq_ignore_ascii_case("python") {
            continue;
        }
        let norm = name.trim().to_ascii_lowercase();
        if !norm.is_empty() && seen.insert(norm.clone()) {
            let version_req = val.as_str().map(str::to_owned).or_else(|| {
                val.get("version")
                    .and_then(TomlValue::as_str)
                    .map(str::to_owned)
            });
            specs.push(DependencySpec {
                ecosystem: Ecosystem::Pypi,
                name: norm,
                version_req,
                manifest_path: manifest_path.to_owned(),
            });
        }
    }
}

fn push_pep508(
    raw: &str,
    manifest_path: &str,
    seen: &mut FxHashSet<String>,
    specs: &mut Vec<DependencySpec>,
) {
    let base = raw.split(';').next().unwrap_or(raw).trim();
    let name_end = base
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_' && c != '.')
        .unwrap_or(base.len());
    let name = base[..name_end].trim().to_ascii_lowercase();
    if name.is_empty() || !seen.insert(name.clone()) {
        return;
    }
    let rest = base[name_end..].trim();
    let version_req = (!rest.is_empty()).then(|| rest.to_owned());
    specs.push(DependencySpec {
        ecosystem: Ecosystem::Pypi,
        name,
        version_req,
        manifest_path: manifest_path.to_owned(),
    });
}

fn latest_release_timestamp(root: &JsonValue, version: &str) -> Option<DateTime<Utc>> {
    let files = root
        .get("releases")
        .and_then(|r| r.get(version))
        .and_then(JsonValue::as_array)?;
    files
        .iter()
        .filter_map(|file| {
            file.get("upload_time_iso_8601")
                .or_else(|| file.get("upload_time"))
                .and_then(JsonValue::as_str)
                .and_then(parse_rfc3339_utc)
        })
        .max()
}
