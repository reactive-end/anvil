//! Version pins from `.tool-versions` (asdf) and `mise.toml`: any tool, any language.

use super::Scan;

pub(super) fn run(scan: &mut Scan<'_>) {
    if let Some(text) = scan.read(".tool-versions") {
        for (name, version) in parse_tool_versions(&text) {
            let source = scan.src(".tool-versions", &name);
            scan.tool(&name, version, source, true);
        }
    }
    for file in ["mise.toml", ".mise.toml"] {
        if let Some(text) = scan.read(file) {
            for (name, version) in parse_mise(&text) {
                let source = scan.src(file, &format!("tools.{name}"));
                scan.tool(&name, version, source, true);
            }
        }
    }
}

/// `nodejs 20.11.0` per line; `#` comments and `system`/`latest` pins carry no version.
fn parse_tool_versions(text: &str) -> Vec<(String, Option<String>)> {
    text.lines()
        .map(|l| l.split('#').next().unwrap_or_default().trim())
        .filter(|l| !l.is_empty())
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let name = words.next()?.to_owned();
            Some((name, words.next().and_then(pin_version)))
        })
        .collect()
}

fn parse_mise(text: &str) -> Vec<(String, Option<String>)> {
    let Ok(doc) = text.parse::<toml::Table>() else { return Vec::new() };
    let Some(tools) = doc.get("tools").and_then(toml::Value::as_table) else { return Vec::new() };
    tools
        .iter()
        .map(|(name, value)| {
            let raw = match value {
                toml::Value::String(s) => Some(s.as_str()),
                toml::Value::Array(a) => a.first().and_then(toml::Value::as_str),
                toml::Value::Table(t) => t.get("version").and_then(toml::Value::as_str),
                _ => None,
            };
            (name.clone(), raw.and_then(pin_version))
        })
        .collect()
}

/// A pin such as `20.11.0`, `v20`, `temurin-17.0.1` or `openjdk-21`; aliases like `lts` give `None`.
pub(super) fn pin_version(raw: &str) -> Option<String> {
    let raw = raw.trim().trim_start_matches('v');
    let numeric = raw.rsplit('-').find(|part| part.starts_with(|c: char| c.is_ascii_digit()))?;
    let version: String = numeric.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
    (!version.is_empty()).then_some(version)
}
