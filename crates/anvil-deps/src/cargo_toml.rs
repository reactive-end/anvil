//! Cargo (`Cargo.toml` + `crates.io/api/v1/crates/<name>`) dependency parser and fetcher.

use anyhow::{Context, Result};
use rustc_hash::FxHashSet;
use serde_json::Value as JsonValue;
use toml::Value as TomlValue;

use crate::http::{fetch_crates_io_json, parse_rfc3339_utc, percent_encode_component};
use crate::model::{DependencySpec, Ecosystem, PackageHealthInfo};

const DEP_TABLES: &[&str] = &["dependencies", "dev-dependencies", "build-dependencies"];

/// Parses crate dependencies from `Cargo.toml` (crate and workspace dependency tables).
///
/// Skips local `path`, `git`, and inherited `workspace = true` entries without an explicit version.
pub fn parse_cargo_toml(manifest_path: &str, content: &str) -> Result<Vec<DependencySpec>> {
    let root: TomlValue = toml::from_str(content)
        .with_context(|| format!("failed to parse TOML in {manifest_path}"))?;

    let mut seen = FxHashSet::default();
    let mut specs = Vec::new();

    for key in DEP_TABLES {
        if let Some(table) = root.get(*key).and_then(TomlValue::as_table) {
            collect_cargo_table(table, manifest_path, &mut seen, &mut specs);
        }
    }

    if let Some(ws_deps) =
        root.get("workspace").and_then(|w| w.get("dependencies")).and_then(TomlValue::as_table)
    {
        collect_cargo_table(ws_deps, manifest_path, &mut seen, &mut specs);
    }

    if let Some(targets) = root.get("target").and_then(TomlValue::as_table) {
        for target_cfg in targets.values().filter_map(TomlValue::as_table) {
            for key in DEP_TABLES {
                if let Some(table) = target_cfg.get(*key).and_then(TomlValue::as_table) {
                    collect_cargo_table(table, manifest_path, &mut seen, &mut specs);
                }
            }
        }
    }

    specs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(specs)
}

/// Names of registry crates a member manifest inherits through `x.workspace = true` (no version of its own).
///
/// These have no requirement in the member manifest, so they can only be audited via the lockfile.
pub fn inherited_workspace_deps(content: &str) -> Vec<String> {
    let Ok(root) = toml::from_str::<TomlValue>(content) else {
        return Vec::new();
    };
    let mut tables: Vec<&toml::map::Map<String, TomlValue>> = Vec::new();
    for key in DEP_TABLES {
        tables.extend(root.get(*key).and_then(TomlValue::as_table));
    }
    if let Some(targets) = root.get("target").and_then(TomlValue::as_table) {
        for target_cfg in targets.values().filter_map(TomlValue::as_table) {
            for key in DEP_TABLES {
                tables.extend(target_cfg.get(*key).and_then(TomlValue::as_table));
            }
        }
    }

    let mut names = Vec::new();
    for (alias, val) in tables.into_iter().flatten() {
        let Some(table) = val.as_table() else {
            continue;
        };
        let inherited = table.get("workspace").and_then(TomlValue::as_bool) == Some(true);
        if !inherited || table.contains_key("version") {
            continue;
        }
        let name = table.get("package").and_then(TomlValue::as_str).unwrap_or(alias).trim();
        if !name.is_empty() && !names.iter().any(|n| n == name) {
            names.push(name.to_owned());
        }
    }
    names
}

/// Fetches crate health metadata from `https://crates.io/api/v1/crates/<package>`.
pub fn fetch_cargo(package: &str) -> Result<PackageHealthInfo> {
    let encoded = percent_encode_component(package);
    let url = format!("https://crates.io/api/v1/crates/{encoded}");
    let body = fetch_crates_io_json(&url)?;
    parse_crates_io_json(package, &body)
}

/// Parses a crates.io `/api/v1/crates/<name>` JSON response into [`PackageHealthInfo`].
pub fn parse_crates_io_json(package: &str, body: &str) -> Result<PackageHealthInfo> {
    let root: JsonValue = serde_json::from_str(body)
        .with_context(|| format!("invalid crates.io JSON for `{package}`"))?;
    let crate_obj = root.get("crate");

    let latest_version = crate_obj
        .and_then(|c| {
            c.get("max_stable_version")
                .or_else(|| c.get("newest_version"))
                .or_else(|| c.get("max_version"))
        })
        .and_then(JsonValue::as_str)
        .map(str::to_owned);

    let versions = root.get("versions").and_then(JsonValue::as_array);
    let latest_ver_obj = versions.and_then(|arr| {
        latest_version
            .as_deref()
            .and_then(|ver| {
                arr.iter().find(|v| v.get("num").and_then(JsonValue::as_str) == Some(ver))
            })
            .or_else(|| arr.first())
    });

    let yanked =
        latest_ver_obj.and_then(|v| v.get("yanked")).and_then(JsonValue::as_bool).unwrap_or(false);

    let last_publish = crate_obj
        .and_then(|c| c.get("updated_at"))
        .and_then(JsonValue::as_str)
        .and_then(parse_rfc3339_utc)
        .or_else(|| {
            latest_ver_obj
                .and_then(|v| v.get("created_at"))
                .and_then(JsonValue::as_str)
                .and_then(parse_rfc3339_utc)
        });

    let repository_url = crate_obj
        .and_then(|c| c.get("repository").or_else(|| c.get("homepage")))
        .and_then(JsonValue::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    Ok(PackageHealthInfo {
        ecosystem: Ecosystem::Cargo,
        name: package.to_owned(),
        latest_version,
        last_publish,
        deprecated: yanked,
        deprecation_message: yanked.then(|| "latest crates.io version is yanked".to_owned()),
        archived: false,
        repository_url,
        vulnerabilities: Vec::new(),
        informational: Vec::new(),
    })
}

fn collect_cargo_table(
    table: &toml::map::Map<String, TomlValue>,
    manifest_path: &str,
    seen: &mut FxHashSet<String>,
    specs: &mut Vec<DependencySpec>,
) {
    for (alias, val) in table {
        let Some((crate_name, version_req)) = extract_cargo_dep(alias, val) else {
            continue;
        };
        if seen.insert(crate_name.clone()) {
            specs.push(DependencySpec {
                ecosystem: Ecosystem::Cargo,
                name: crate_name,
                version_req,
                manifest_path: manifest_path.to_owned(),
                resolved_version: None,
                lockfile: None,
            });
        }
    }
}

fn extract_cargo_dep(alias: &str, val: &TomlValue) -> Option<(String, Option<String>)> {
    if let Some(ver) = val.as_str() {
        return Some((alias.trim().to_owned(), Some(ver.to_owned())));
    }
    let table = val.as_table()?;
    if table.contains_key("path") || table.contains_key("git") {
        return None;
    }
    let version = table.get("version").and_then(TomlValue::as_str).map(str::to_owned);
    if table.get("workspace").and_then(TomlValue::as_bool) == Some(true) && version.is_none() {
        return None;
    }
    let name = table.get("package").and_then(TomlValue::as_str).unwrap_or(alias).trim().to_owned();
    (!name.is_empty()).then_some((name, version))
}
