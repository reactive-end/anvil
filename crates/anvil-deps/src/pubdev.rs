//! pub.dev (`pubspec.yaml` + `pub.dev/api/packages/<pkg>`) parser and fetcher.

use anyhow::{Context, Result};
use rustc_hash::FxHashSet;
use serde_json::Value;

use crate::http::{fetch_json_text, parse_rfc3339_utc, percent_encode_component};
use crate::model::{DependencySpec, Ecosystem, PackageHealthInfo};

/// Parses hosted package dependencies from `pubspec.yaml`.
///
/// Skips local `path:`, `git:`, and `sdk:` entries (such as `flutter: sdk: flutter`).
pub fn parse_pubspec_yaml(manifest_path: &str, content: &str) -> Result<Vec<DependencySpec>> {
    let mut seen = FxHashSet::default();
    let mut specs: Vec<DependencySpec> = Vec::new();
    let mut in_deps = false;
    let mut skip_nested_indent: Option<usize> = None;

    for raw_line in content.lines() {
        let line = strip_yaml_comment(raw_line);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let indent = raw_line.len() - raw_line.trim_start().len();

        if indent == 0 {
            skip_nested_indent = None;
            let key = trimmed.trim_end_matches(':');
            in_deps = matches!(key, "dependencies" | "dev_dependencies");
            continue;
        }
        if !in_deps {
            continue;
        }
        if let Some(pkg_indent) = skip_nested_indent {
            if indent > pkg_indent {
                if matches_local_or_sdk_key(trimmed)
                    && let Some(last) = specs.last()
                {
                    let name = last.name.clone();
                    specs.pop();
                    seen.remove(&name);
                }
                continue;
            }
            skip_nested_indent = None;
        }

        if let Some((name, ver)) = parse_yaml_kv(trimmed) {
            if seen.insert(name.clone()) {
                specs.push(DependencySpec {
                    ecosystem: Ecosystem::PubDev,
                    name,
                    version_req: ver,
                    manifest_path: manifest_path.to_owned(),
                    resolved_version: None,
                    lockfile: None,
                });
            }
            skip_nested_indent = Some(indent);
        }
    }

    specs.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(specs)
}

/// Fetches package metadata from `https://pub.dev/api/packages/<package>`.
pub fn fetch_pubdev(package: &str) -> Result<PackageHealthInfo> {
    let encoded = percent_encode_component(package);
    let url = format!("https://pub.dev/api/packages/{encoded}");
    let body = fetch_json_text(&url)?;
    parse_pubdev_registry_json(package, &body)
}

/// Parses a pub.dev JSON API payload into [`PackageHealthInfo`].
pub fn parse_pubdev_registry_json(package: &str, body: &str) -> Result<PackageHealthInfo> {
    let root: Value = serde_json::from_str(body)
        .with_context(|| format!("invalid pub.dev JSON for `{package}`"))?;

    let discontinued = root.get("isDiscontinued").and_then(Value::as_bool).unwrap_or(false);
    let replaced_by = root.get("replacedBy").and_then(Value::as_str).filter(|s| !s.is_empty());

    let deprecation_message = if discontinued {
        Some(match replaced_by {
            Some(rep) => format!("discontinued; replaced by {rep}"),
            None => "package is marked discontinued on pub.dev".to_owned(),
        })
    } else {
        None
    };

    let latest = root.get("latest");
    let latest_version =
        latest.and_then(|l| l.get("version")).and_then(Value::as_str).map(str::to_owned);
    let last_publish =
        latest.and_then(|l| l.get("published")).and_then(Value::as_str).and_then(parse_rfc3339_utc);
    let repository_url = latest
        .and_then(|l| l.get("pubspec"))
        .and_then(|p| p.get("repository").or_else(|| p.get("homepage")))
        .and_then(Value::as_str)
        .map(str::to_owned);

    Ok(PackageHealthInfo {
        ecosystem: Ecosystem::PubDev,
        name: package.to_owned(),
        latest_version,
        last_publish,
        deprecated: discontinued,
        deprecation_message,
        archived: discontinued,
        repository_url,
        vulnerabilities: Vec::new(),
        informational: Vec::new(),
    })
}

fn strip_yaml_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    for (idx, ch) in line.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => return &line[..idx],
            _ => {}
        }
    }
    line
}

fn parse_yaml_kv(trimmed: &str) -> Option<(String, Option<String>)> {
    let (raw_key, raw_val) = trimmed.split_once(':')?;
    let name = raw_key.trim().trim_matches(['"', '\'']).to_owned();
    if name.is_empty() || name.contains(char::is_whitespace) {
        return None;
    }
    let val = raw_val.trim().trim_matches(['"', '\'']);
    let version = (!val.is_empty()).then(|| val.to_owned());
    Some((name, version))
}

fn matches_local_or_sdk_key(trimmed: &str) -> bool {
    trimmed.starts_with("sdk:") || trimmed.starts_with("path:") || trimmed.starts_with("git:")
}
