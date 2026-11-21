//! npm (`package.json` + `registry.npmjs.org`) dependency parser and fetcher.

use anyhow::{Context, Result};
use rustc_hash::FxHashSet;
use serde_json::Value;

use crate::http::{fetch_json_text, parse_rfc3339_utc, percent_encode_component};
use crate::model::{DependencySpec, Ecosystem, PackageHealthInfo};

const DEP_SECTIONS: &[&str] = &[
    "dependencies",
    "devDependencies",
    "peerDependencies",
    "optionalDependencies",
];

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

/// Fetches package health metadata from `https://registry.npmjs.org/<package>`.
pub fn fetch_npm(package: &str) -> Result<PackageHealthInfo> {
    let encoded = percent_encode_component(package);
    let url = format!("https://registry.npmjs.org/{encoded}");
    let body = fetch_json_text(&url)?;
    parse_npm_registry_json(package, &body)
}

/// Parses an npm registry JSON payload into [`PackageHealthInfo`].
pub fn parse_npm_registry_json(package: &str, body: &str) -> Result<PackageHealthInfo> {
    let root: Value = serde_json::from_str(body)
        .with_context(|| format!("invalid npm registry JSON for `{package}`"))?;

    let latest_version = root
        .get("dist-tags")
        .and_then(|dt| dt.get("latest"))
        .and_then(Value::as_str)
        .map(str::to_owned);

    let last_publish = latest_version
        .as_deref()
        .and_then(|ver| root.get("time").and_then(|t| t.get(ver)))
        .or_else(|| root.get("time").and_then(|t| t.get("modified")))
        .and_then(Value::as_str)
        .and_then(parse_rfc3339_utc);

    let deprecation_message = latest_version
        .as_deref()
        .and_then(|ver| root.get("versions").and_then(|v| v.get(ver)))
        .and_then(|v| v.get("deprecated"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    let repository_url = root
        .get("repository")
        .and_then(|repo| match repo {
            Value::String(s) => Some(s.clone()),
            Value::Object(map) => map.get("url").and_then(Value::as_str).map(str::to_owned),
            _ => None,
        });

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
