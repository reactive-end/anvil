//! Per-format lockfile parsers: each returns `package name -> exact locked versions`.

use rustc_hash::FxHashMap;
use serde_json::Value as JsonValue;
use toml::Value as TomlValue;

/// Exact versions per package name, in lockfile order without duplicates.
pub(super) type LockedVersions = FxHashMap<String, Vec<String>>;

fn add(map: &mut LockedVersions, name: &str, version: &str) {
    let name = name.trim();
    let version = version.trim();
    if name.is_empty() || !version.as_bytes().first().is_some_and(u8::is_ascii_digit) {
        return;
    }
    let versions = map.entry(name.to_owned()).or_default();
    if !versions.iter().any(|v| v == version) {
        versions.push(version.to_owned());
    }
}

/// `Cargo.lock`: registry and git packages only (workspace members and path crates have no `source`).
pub(super) fn parse_cargo_lock(content: &str) -> LockedVersions {
    let mut out = LockedVersions::default();
    let Ok(root) = toml::from_str::<TomlValue>(content) else {
        return out;
    };
    for pkg in root.get("package").and_then(TomlValue::as_array).into_iter().flatten() {
        let (Some(name), Some(version)) = (
            pkg.get("name").and_then(TomlValue::as_str),
            pkg.get("version").and_then(TomlValue::as_str),
        ) else {
            continue;
        };
        if pkg.get("source").and_then(TomlValue::as_str).is_some() {
            add(&mut out, name, version);
        }
    }
    out
}

/// `poetry.lock` / `uv.lock`: `[[package]]` tables with `name` and `version`; names PEP 503 normalised.
pub(super) fn parse_python_lock(content: &str) -> LockedVersions {
    let mut out = LockedVersions::default();
    let Ok(root) = toml::from_str::<TomlValue>(content) else {
        return out;
    };
    for pkg in root.get("package").and_then(TomlValue::as_array).into_iter().flatten() {
        if let (Some(name), Some(version)) = (
            pkg.get("name").and_then(TomlValue::as_str),
            pkg.get("version").and_then(TomlValue::as_str),
        ) {
            add(&mut out, &normalize_pypi_name(name), version);
        }
    }
    out
}

/// PEP 503 name normalisation (lowercase, runs of `-_.` collapse to `-`).
pub(super) fn normalize_pypi_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.trim().chars() {
        if matches!(c, '-' | '_' | '.') {
            if !out.ends_with('-') {
                out.push('-');
            }
        } else {
            out.push(c.to_ascii_lowercase());
        }
    }
    out
}

/// `composer.lock`: `packages` and `packages-dev` entries; a leading `v` is stripped (`v1.2.3`).
pub(super) fn parse_composer_lock(content: &str) -> LockedVersions {
    let mut out = LockedVersions::default();
    let Ok(root) = serde_json::from_str::<JsonValue>(content) else {
        return out;
    };
    for section in ["packages", "packages-dev"] {
        for pkg in root.get(section).and_then(JsonValue::as_array).into_iter().flatten() {
            if let (Some(name), Some(version)) = (
                pkg.get("name").and_then(JsonValue::as_str),
                pkg.get("version").and_then(JsonValue::as_str),
            ) {
                add(&mut out, &name.to_ascii_lowercase(), version.trim_start_matches('v'));
            }
        }
    }
    out
}

/// `package-lock.json` / `npm-shrinkwrap.json` (v1 `dependencies`, v2/v3 `packages`).
///
/// `member_dir` is the manifest directory relative to the lockfile (`""` for the lock's own
/// project). Workspace-member installs (`<member>/node_modules/<name>`) win over hoisted ones.
pub(super) fn parse_npm_lock(content: &str, member_dir: &str) -> LockedVersions {
    let mut out = LockedVersions::default();
    let Ok(root) = serde_json::from_str::<JsonValue>(content) else {
        return out;
    };

    if let Some(packages) = root.get("packages").and_then(JsonValue::as_object) {
        let member_prefix = (!member_dir.is_empty()).then(|| format!("{member_dir}/node_modules/"));
        let mut hoisted = LockedVersions::default();
        let mut local = LockedVersions::default();
        for (key, entry) in packages {
            let Some(version) = entry.get("version").and_then(JsonValue::as_str) else {
                continue;
            };
            if let Some(name) = top_level_module(key, "node_modules/") {
                add(&mut hoisted, name, version);
            } else if let Some(prefix) = &member_prefix
                && let Some(name) = top_level_module(key, prefix)
            {
                add(&mut local, name, version);
            }
        }
        out = hoisted;
        out.extend(local);
        return out;
    }

    for (name, entry) in
        root.get("dependencies").and_then(JsonValue::as_object).into_iter().flatten()
    {
        if let Some(version) = entry.get("version").and_then(JsonValue::as_str) {
            add(&mut out, name, version);
        }
    }
    out
}

/// Returns `<name>` for keys shaped like `<prefix><name>` where `name` has no nested `node_modules`.
fn top_level_module<'a>(key: &'a str, prefix: &str) -> Option<&'a str> {
    let name = key.strip_prefix(prefix)?;
    (!name.is_empty() && !name.contains("node_modules/")).then_some(name)
}

/// `yarn.lock` (classic `version "x"` and berry `version: x` blocks).
pub(super) fn parse_yarn_lock(content: &str) -> LockedVersions {
    let mut out = LockedVersions::default();
    let mut current: Vec<String> = Vec::new();
    for line in content.lines() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            current = line
                .trim_end()
                .trim_end_matches(':')
                .split(',')
                .filter_map(|spec| yarn_spec_name(spec.trim().trim_matches(['"', '\''])))
                .collect();
        } else if let Some(rest) = line.trim_start().strip_prefix("version")
            && !current.is_empty()
        {
            let version = rest.trim_start_matches([':', ' ']).trim().trim_matches(['"', '\'']);
            for name in &current {
                add(&mut out, name, version);
            }
        }
    }
    out
}

fn yarn_spec_name(spec: &str) -> Option<String> {
    let at = spec.get(1..)?.find('@')? + 1;
    Some(spec[..at].to_owned())
}

/// `pnpm-lock.yaml`: keys of the top-level `packages:` map shaped like `name@1.2.3` (v6+/v9).
pub(super) fn parse_pnpm_lock(content: &str) -> LockedVersions {
    let mut out = LockedVersions::default();
    let mut in_packages = false;
    for line in content.lines() {
        if !line.starts_with(char::is_whitespace) && !line.trim().is_empty() {
            in_packages = line.trim_end() == "packages:";
            continue;
        }
        if !in_packages || line.len() - line.trim_start().len() != 2 {
            continue;
        }
        let key = line.trim().trim_end_matches(':').trim_matches(['"', '\'']);
        let key = key.trim_start_matches('/');
        let key = key.split('(').next().unwrap_or(key);
        if let Some(at) = key.rfind('@')
            && at > 0
        {
            add(&mut out, &key[..at], &key[at + 1..]);
        }
    }
    out
}

/// `pubspec.lock`: `packages:` map with `version: "x"` under each package.
pub(super) fn parse_pubspec_lock(content: &str) -> LockedVersions {
    let mut out = LockedVersions::default();
    let mut in_packages = false;
    let mut current: Option<String> = None;
    for line in content.lines() {
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        match indent {
            0 => {
                in_packages = line.trim_end() == "packages:";
                current = None;
            }
            2 if in_packages => {
                current =
                    Some(line.trim().trim_end_matches(':').trim_matches(['"', '\'']).to_owned());
            }
            4 if in_packages => {
                if let Some(rest) = line.trim().strip_prefix("version:")
                    && let Some(name) = &current
                {
                    add(&mut out, name, rest.trim().trim_matches(['"', '\'']));
                }
            }
            _ => {}
        }
    }
    out
}

/// `gradle.lockfile`: `group:artifact:version=configurations` lines.
pub(super) fn parse_gradle_lockfile(content: &str) -> LockedVersions {
    let mut out = LockedVersions::default();
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let coord = line.split('=').next().unwrap_or(line);
        let mut parts = coord.splitn(3, ':');
        if let (Some(group), Some(artifact), Some(version)) =
            (parts.next(), parts.next(), parts.next())
        {
            add(&mut out, &format!("{group}:{artifact}"), version);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(map: &LockedVersions, name: &str) -> Vec<String> {
        map.get(name).cloned().unwrap_or_default()
    }

    #[test]
    fn cargo_lock_keeps_every_registry_version_and_skips_path_crates() {
        let lock = r#"
version = 4

[[package]]
name = "anvil-core"
version = "0.1.0"

[[package]]
name = "syn"
version = "1.0.109"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "syn"
version = "2.0.90"
source = "registry+https://github.com/rust-lang/crates.io-index"
"#;
        let map = parse_cargo_lock(lock);
        assert_eq!(v(&map, "syn"), vec!["1.0.109", "2.0.90"]);
        assert!(!map.contains_key("anvil-core"));
    }

    #[test]
    fn npm_lock_v3_prefers_member_install_over_hoisted_and_ignores_nested() {
        let lock = r#"{
          "lockfileVersion": 3,
          "packages": {
            "": { "name": "root" },
            "node_modules/lodash": { "version": "4.17.21" },
            "node_modules/@scope/pkg": { "version": "2.0.0" },
            "node_modules/a/node_modules/lodash": { "version": "3.0.0" },
            "apps/web/node_modules/lodash": { "version": "4.17.10" }
          }
        }"#;
        let root = parse_npm_lock(lock, "");
        assert_eq!(v(&root, "lodash"), vec!["4.17.21"]);
        assert_eq!(v(&root, "@scope/pkg"), vec!["2.0.0"]);
        let member = parse_npm_lock(lock, "apps/web");
        assert_eq!(v(&member, "lodash"), vec!["4.17.10"]);
    }

    #[test]
    fn npm_lock_v1_reads_top_level_dependencies() {
        let lock =
            r#"{ "lockfileVersion": 1, "dependencies": { "left-pad": { "version": "1.3.0" } } }"#;
        assert_eq!(v(&parse_npm_lock(lock, ""), "left-pad"), vec!["1.3.0"]);
    }

    #[test]
    fn composer_lock_reads_both_sections_and_strips_v_prefix() {
        let lock = r#"{
          "packages": [ { "name": "Monolog/Monolog", "version": "3.5.0" },
                        { "name": "acme/dev-thing", "version": "dev-main" } ],
          "packages-dev": [ { "name": "phpunit/phpunit", "version": "v10.5.1" } ]
        }"#;
        let map = parse_composer_lock(lock);
        assert_eq!(v(&map, "monolog/monolog"), vec!["3.5.0"]);
        assert_eq!(v(&map, "phpunit/phpunit"), vec!["10.5.1"]);
        assert!(!map.contains_key("acme/dev-thing"));
    }

    #[test]
    fn python_lock_normalises_names() {
        let lock = "[[package]]\nname = \"Typing_Extensions\"\nversion = \"4.9.0\"\n";
        assert_eq!(v(&parse_python_lock(lock), "typing-extensions"), vec!["4.9.0"]);
    }

    #[test]
    fn yarn_classic_and_berry_blocks_resolve() {
        let classic = "# yarn lockfile v1\n\nlodash@^4.17.0, lodash@^4.17.21:\n  version \"4.17.21\"\n  resolved \"x\"\n\n\"@babel/core@^7.0.0\":\n  version \"7.23.0\"\n";
        let map = parse_yarn_lock(classic);
        assert_eq!(v(&map, "lodash"), vec!["4.17.21"]);
        assert_eq!(v(&map, "@babel/core"), vec!["7.23.0"]);
        let berry =
            "\"left-pad@npm:^1.0.0\":\n  version: 1.3.0\n  resolution: \"left-pad@npm:1.3.0\"\n";
        assert_eq!(v(&parse_yarn_lock(berry), "left-pad"), vec!["1.3.0"]);
    }

    #[test]
    fn pnpm_lock_reads_package_keys() {
        let lock = "lockfileVersion: '9.0'\n\nimporters:\n  .:\n    dependencies:\n      x: 1\n\npackages:\n\n  '@scope/pkg@2.1.0':\n    resolution: {}\n\n  react-dom@18.2.0(react@18.2.0):\n    resolution: {}\n\nsnapshots:\n  other@9.9.9:\n    x: 1\n";
        let map = parse_pnpm_lock(lock);
        assert_eq!(v(&map, "@scope/pkg"), vec!["2.1.0"]);
        assert_eq!(v(&map, "react-dom"), vec!["18.2.0"]);
        assert!(!map.contains_key("other"));
    }

    #[test]
    fn pubspec_lock_and_gradle_lockfile_resolve() {
        let pubspec = "packages:\n  http:\n    dependency: \"direct main\"\n    description:\n      name: http\n    source: hosted\n    version: \"1.2.0\"\nsdks:\n  dart: \">=3.0.0\"\n";
        assert_eq!(v(&parse_pubspec_lock(pubspec), "http"), vec!["1.2.0"]);
        let gradle = "# comment\ncom.google.guava:guava:32.1.3-jre=compileClasspath,runtimeClasspath\nempty=annotationProcessor\n";
        assert_eq!(v(&parse_gradle_lockfile(gradle), "com.google.guava:guava"), vec!["32.1.3-jre"]);
    }
}
