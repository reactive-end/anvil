//! Composer (`composer.json` + Packagist API) dependency parser and fetcher.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use rustc_hash::FxHashSet;
use serde_json::Value;

use crate::http::{fetch_text_with_accept, parse_rfc3339_utc, percent_encode_component};
use crate::model::{DependencySpec, Ecosystem, PackageHealthInfo};

const DEP_SECTIONS: &[&str] = &["require", "require-dev"];

/// Parses Packagist package dependencies from `composer.json`, skipping platform requirements.
pub fn parse_composer_json(manifest_path: &str, content: &str) -> Result<Vec<DependencySpec>> {
    let clean = content.strip_prefix('\u{feff}').unwrap_or(content);
    let root: Value = serde_json::from_str(clean)
        .with_context(|| format!("failed to parse JSON in {manifest_path}"))?;

    let mut seen = FxHashSet::default();
    let mut specs = Vec::new();

    for section in DEP_SECTIONS {
        let Some(table) = root.get(*section).and_then(Value::as_object) else {
            continue;
        };
        for (name, version) in table {
            if is_platform_requirement(name) || !name.contains('/') {
                continue;
            }
            if seen.insert(name.clone()) {
                specs.push(DependencySpec {
                    ecosystem: Ecosystem::Composer,
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

fn is_platform_requirement(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "php"
            | "php-64bit"
            | "php-ipv6"
            | "php-zts"
            | "php-debug"
            | "hhvm"
            | "composer"
            | "composer-plugin-api"
            | "composer-runtime-api"
    ) || lower.starts_with("ext-")
        || lower.starts_with("lib-")
}

/// Fetches package health metadata from `https://packagist.org/packages/<vendor>/<package>.json`.
pub fn fetch_composer(package: &str) -> Result<PackageHealthInfo> {
    let Some((vendor, name)) = package.split_once('/') else {
        bail!("invalid Composer package name `{package}` (expected `vendor/package`)");
    };
    let enc_vendor = percent_encode_component(vendor);
    let enc_name = percent_encode_component(name);
    let url = format!("https://packagist.org/packages/{enc_vendor}/{enc_name}.json");
    let body = fetch_text_with_accept(&url, "application/json")?;
    parse_packagist_json(package, &body)
}

/// Parses a Packagist `/packages/<vendor>/<name>.json` response into [`PackageHealthInfo`].
pub fn parse_packagist_json(package: &str, body: &str) -> Result<PackageHealthInfo> {
    let root: Value = serde_json::from_str(body)
        .with_context(|| format!("invalid Packagist JSON for `{package}`"))?;
    let pkg = root.get("package").unwrap_or(&root);

    let (deprecated, deprecation_message) = parse_abandoned_field(pkg.get("abandoned"));
    let repository_url = pkg
        .get("repository")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    let (latest_version, last_publish) = extract_latest_release(pkg.get("versions"));

    Ok(PackageHealthInfo {
        ecosystem: Ecosystem::Composer,
        name: package.to_owned(),
        latest_version,
        last_publish,
        deprecated,
        deprecation_message,
        archived: false,
        repository_url,
        vulnerabilities: Vec::new(),
    })
}

fn parse_abandoned_field(val: Option<&Value>) -> (bool, Option<String>) {
    match val {
        Some(Value::Bool(true)) => (
            true,
            Some("Package is marked abandoned on Packagist".to_owned()),
        ),
        Some(Value::String(replacement)) if !replacement.trim().is_empty() => (
            true,
            Some(format!(
                "Package is abandoned on Packagist; use `{}` instead",
                replacement.trim()
            )),
        ),
        _ => (false, None),
    }
}

fn extract_latest_release(versions: Option<&Value>) -> (Option<String>, Option<DateTime<Utc>>) {
    let Some(map) = versions.and_then(Value::as_object) else {
        return (None, None);
    };

    let mut best_stable: Option<(DateTime<Utc>, String)> = None;
    let mut best_any: Option<(DateTime<Utc>, String)> = None;

    for (ver_key, entry) in map {
        let ver = entry
            .get("version")
            .and_then(Value::as_str)
            .unwrap_or(ver_key.as_str());
        let Some(ts) = entry
            .get("time")
            .and_then(Value::as_str)
            .and_then(parse_rfc3339_utc)
        else {
            continue;
        };
        let is_dev = is_dev_version(ver);
        if !is_dev && best_stable.as_ref().is_none_or(|(prev, _)| ts > *prev) {
            best_stable = Some((ts, ver.to_owned()));
        }
        if best_any.as_ref().is_none_or(|(prev, _)| ts > *prev) {
            best_any = Some((ts, ver.to_owned()));
        }
    }

    match best_stable.or(best_any) {
        Some((ts, ver)) => (Some(ver), Some(ts)),
        None => (None, None),
    }
}

fn is_dev_version(ver: &str) -> bool {
    let lower = ver.to_ascii_lowercase();
    lower.starts_with("dev-") || lower.ends_with("-dev")
}
