//! npm (`package.json` + abbreviated `registry.npmjs.org`) dependency parser and fetcher.

use anyhow::{Context, Result};
use rustc_hash::FxHashSet;
use serde_json::Value;

use crate::http::{fetch_text_with_accept, parse_rfc3339_utc, percent_encode_component};
use crate::model::{DependencySpec, Ecosystem, PackageHealthInfo};

const DEP_SECTIONS: &[&str] = &[
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];
const NPM_ABBREVIATED_ACCEPT: &str = "application/vnd.npm.install-v1+json";

/// Parses dependency entries from a `package.json` file.
pub fn parse_package_json(manifest_path: &str, content: &str) -> Result<Vec<DependencySpec>> {
    let root: Value = serde_json::from_str(content)
        .with_context(|| format!("failed to parse JSON in {manifest_path}"))?;

    let mut seen = FxHashSet::default();
    let mut specs = Vec::new();

    for section in DEP_SECTIONS {
        let Some(table) = root.get(*section).and_then(Value::as_object) else {
            continue;
        };
        for (name, version) in table {
            if seen.insert(name.clone()) {
                specs.push(DependencySpec {
                    ecosystem: Ecosystem::Npm,
                    name: name.clone(),
                    version_req: version.as_str().map(str::to_owned),
                    manifest_path: manifest_path.to_owned(),
                });
            }
        }
    }

    specs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(specs)
}

/// Fetches abbreviated npm metadata (`install-v1+json`) instead of multi-MB full packuments.
///
/// Falls back to `/<pkg>/latest` for unscoped packages if the abbreviated packument fails.
pub fn fetch_npm(package: &str) -> Result<PackageHealthInfo> {
    let encoded = percent_encode_component(package);
    let url = format!("https://registry.npmjs.org/{encoded}");
    match fetch_text_with_accept(&url, NPM_ABBREVIATED_ACCEPT) {
        Ok(body) => parse_npm_registry_json(package, &body),
        Err(abbr_err) => {
            if !package.starts_with('@') {
                let latest_url = format!("https://registry.npmjs.org/{encoded}/latest");
                if let Ok(body) = fetch_text_with_accept(&latest_url, "application/json") {
                    return parse_npm_registry_json(package, &body);
                }
            }
            Err(abbr_err)
        }
    }
}

/// Parses an npm abbreviated (`install-v1+json`), `/latest`, or full registry JSON payload.
pub fn parse_npm_registry_json(package: &str, body: &str) -> Result<PackageHealthInfo> {
    let root: Value = serde_json::from_str(body)
        .with_context(|| format!("invalid npm registry JSON for `{package}`"))?;

    let latest_version = root
        .get("dist-tags")
        .and_then(|dt| dt.get("latest"))
        .and_then(Value::as_str)
        .or_else(|| root.get("version").and_then(Value::as_str))
        .map(str::to_owned);

    let last_publish = latest_version
        .as_deref()
        .and_then(|ver| root.get("time").and_then(|t| t.get(ver)))
        .or_else(|| root.get("time").and_then(|t| t.get("modified")))
        .or_else(|| root.get("modified"))
        .and_then(Value::as_str)
        .and_then(parse_rfc3339_utc);

    let latest_obj = latest_version
        .as_deref()
        .and_then(|ver| root.get("versions").and_then(|v| v.get(ver)));

    let deprecation_message = latest_obj
        .and_then(|v| v.get("deprecated"))
        .or_else(|| root.get("deprecated"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    let repository_url = extract_repo_url(root.get("repository"))
        .or_else(|| latest_obj.and_then(|v| extract_repo_url(v.get("repository"))));

    Ok(PackageHealthInfo {
        ecosystem: Ecosystem::Npm,
        name: package.to_owned(),
        latest_version,
        last_publish,
        deprecated: deprecation_message.is_some(),
        deprecation_message,
        archived: false,
        repository_url,
    })
}

fn extract_repo_url(repo: Option<&Value>) -> Option<String> {
    match repo? {
        Value::String(s) => Some(s.clone()),
        Value::Object(map) => map.get("url").and_then(Value::as_str).map(str::to_owned),
        _ => None,
    }
}
