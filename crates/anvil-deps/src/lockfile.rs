//! Resolves manifest requirements to the exact versions pinned by the lockfile next to them.
//!
//! OSV only answers precisely for a concrete version; a requirement such as `regex = "1"` would be
//! read as "any 1.x" and surface advisories that cannot affect the version actually built.
//!
//! Supported lockfiles: `Cargo.lock`, `package-lock.json`/`npm-shrinkwrap.json`, `pnpm-lock.yaml`,
//! `yarn.lock` (classic and berry), `composer.lock`, `pubspec.lock`, `poetry.lock`/`uv.lock`, and
//! `gradle.lockfile`. `requirements*.txt` has no lockfile format and stays unresolved.

mod formats;

use std::fs;
use std::path::{Path, PathBuf};

use crate::cargo_toml::inherited_workspace_deps;
use crate::model::{DependencySpec, Ecosystem};
use formats::{
    LockedVersions, normalize_pypi_name, parse_cargo_lock, parse_composer_lock,
    parse_gradle_lockfile, parse_npm_lock, parse_pnpm_lock, parse_pubspec_lock, parse_python_lock,
    parse_yarn_lock,
};

const MAX_LOCKFILE_BYTES: u64 = 64 * 1024 * 1024;

/// Locked versions of one lockfile, scoped to the manifest being audited.
struct LockIndex {
    /// Repo-relative path (forward slashes) shown in findings.
    rel_path: String,
    ecosystem: Ecosystem,
    versions: LockedVersions,
}

impl LockIndex {
    fn versions_of(&self, name: &str) -> &[String] {
        let key = if self.ecosystem == Ecosystem::Pypi {
            normalize_pypi_name(name)
        } else {
            name.to_owned()
        };
        self.versions.get(&key).map_or(&[], Vec::as_slice)
    }
}

/// Lockfile names (priority order) that can pin the dependencies of `manifest_name`.
fn lock_candidates(manifest_name: &str) -> &'static [&'static str] {
    match manifest_name {
        "Cargo.toml" => &["Cargo.lock"],
        "package.json" => {
            &["package-lock.json", "npm-shrinkwrap.json", "pnpm-lock.yaml", "yarn.lock"]
        }
        "composer.json" => &["composer.lock"],
        "pubspec.yaml" => &["pubspec.lock"],
        "pyproject.toml" => &["poetry.lock", "uv.lock"],
        "build.gradle" | "build.gradle.kts" | "libs.versions.toml" => &["gradle.lockfile"],
        _ => &[],
    }
}

/// Walks from the manifest directory up to `repo_root` (so workspace members find the root lock).
fn find_lockfile(repo_root: &Path, manifest: &Path, candidates: &[&str]) -> Option<PathBuf> {
    let start = manifest.parent()?;
    let bounded = start.starts_with(repo_root);
    for dir in start.ancestors() {
        for name in candidates {
            let path = dir.join(name);
            if path.is_file() {
                return Some(path);
            }
        }
        if !bounded || dir == repo_root {
            break;
        }
    }
    None
}

fn load_index(repo_root: &Path, manifest: &Path, ecosystem: Ecosystem) -> Option<LockIndex> {
    let name = manifest.file_name()?.to_str()?;
    let lock_path = find_lockfile(repo_root, manifest, lock_candidates(name))?;
    if fs::metadata(&lock_path).ok()?.len() > MAX_LOCKFILE_BYTES {
        return None;
    }
    let content = fs::read_to_string(&lock_path).ok()?;
    let lock_name = lock_path.file_name()?.to_str()?;
    let lock_dir = lock_path.parent()?;
    let member_dir = manifest
        .parent()
        .and_then(|dir| dir.strip_prefix(lock_dir).ok())
        .map(|rel| rel.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();

    let versions = match lock_name {
        "Cargo.lock" => parse_cargo_lock(&content),
        "package-lock.json" | "npm-shrinkwrap.json" => parse_npm_lock(&content, &member_dir),
        "pnpm-lock.yaml" => parse_pnpm_lock(&content),
        "yarn.lock" => parse_yarn_lock(&content),
        "composer.lock" => parse_composer_lock(&content),
        "pubspec.lock" => parse_pubspec_lock(&content),
        "poetry.lock" | "uv.lock" => parse_python_lock(&content),
        "gradle.lockfile" => parse_gradle_lockfile(&content),
        _ => return None,
    };
    let rel_path = lock_path
        .strip_prefix(repo_root)
        .unwrap_or(&lock_path)
        .to_string_lossy()
        .replace('\\', "/");
    Some(LockIndex { rel_path, ecosystem, versions })
}

/// Pins each spec to the exact version(s) in the lockfile next to `manifest`.
///
/// A package locked at several versions yields one spec per version so each is audited. Specs with no
/// lock entry (or no lockfile at all) are returned unchanged, i.e. still unresolved. For `Cargo.toml`
/// manifests, `x.workspace = true` dependencies are added when the lockfile pins them.
pub(crate) fn resolve_specs(
    repo_root: &Path,
    manifest: &Path,
    content: &str,
    mut specs: Vec<DependencySpec>,
) -> Vec<DependencySpec> {
    let Some(ecosystem) = ecosystem_of(manifest) else {
        return specs;
    };
    let Some(index) = load_index(repo_root, manifest, ecosystem) else {
        return specs;
    };

    if ecosystem == Ecosystem::Cargo {
        let manifest_path = manifest
            .strip_prefix(repo_root)
            .unwrap_or(manifest)
            .to_string_lossy()
            .replace('\\', "/");
        for name in inherited_workspace_deps(content) {
            if !index.versions_of(&name).is_empty() && !specs.iter().any(|s| s.name == name) {
                specs.push(DependencySpec {
                    ecosystem,
                    name,
                    version_req: None,
                    manifest_path: manifest_path.clone(),
                    resolved_version: None,
                    lockfile: None,
                });
            }
        }
    }

    let mut out = Vec::with_capacity(specs.len());
    for spec in specs {
        let locked = index.versions_of(&spec.name);
        if locked.is_empty() {
            out.push(spec);
            continue;
        }
        for version in locked {
            out.push(DependencySpec {
                resolved_version: Some(version.clone()),
                lockfile: Some(index.rel_path.clone()),
                ..spec.clone()
            });
        }
    }
    out
}

fn ecosystem_of(manifest: &Path) -> Option<Ecosystem> {
    match manifest.file_name()?.to_str()? {
        "Cargo.toml" => Some(Ecosystem::Cargo),
        "package.json" => Some(Ecosystem::Npm),
        "composer.json" => Some(Ecosystem::Composer),
        "pubspec.yaml" => Some(Ecosystem::PubDev),
        "pyproject.toml" => Some(Ecosystem::Pypi),
        "build.gradle" | "build.gradle.kts" | "libs.versions.toml" => Some(Ecosystem::Maven),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rustc_hash::FxHashMap;
    use serde_json::Value;
    use tempfile::TempDir;

    use crate::audit::parse_manifest;
    use crate::osv::query_osv_batch_with;

    use super::*;

    fn write(root: &Path, rel: &str, content: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
        fs::write(path, content).expect("write");
    }

    fn version_key(v: &str) -> Vec<u32> {
        v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
    }

    const REGISTRY: &str = "registry+https://github.com/rust-lang/crates.io-index";

    fn cargo_workspace(tmp: &Path, locked: &[(&str, &str)]) {
        write(
            tmp,
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/app\"]\n\n[workspace.dependencies]\nregex = \"1\"\n",
        );
        write(
            tmp,
            "crates/app/Cargo.toml",
            "[package]\nname = \"app\"\nversion = \"0.1.0\"\n\n[dependencies]\nregex.workspace = true\n",
        );
        let mut lock =
            String::from("version = 4\n\n[[package]]\nname = \"app\"\nversion = \"0.1.0\"\n");
        for (name, version) in locked {
            lock.push_str(&format!(
                "\n[[package]]\nname = \"{name}\"\nversion = \"{version}\"\nsource = \"{REGISTRY}\"\n"
            ));
        }
        write(tmp, "Cargo.lock", &lock);
    }

    /// Fake OSV: `RUSTSEC-2022-0013` only affects regex `< 1.5.5`; records every queried version.
    fn fake_osv(specs: &[DependencySpec]) -> (FxHashMap<usize, Vec<String>>, Vec<String>) {
        let mut queried = Vec::new();
        let found = query_osv_batch_with(specs, |body| {
            let parsed: Value = serde_json::from_str(body)?;
            let mut results = Vec::new();
            for query in parsed["queries"].as_array().expect("queries") {
                let version = query["version"].as_str().expect("version").to_owned();
                let affected = version_key(&version) < version_key("1.5.5");
                queried
                    .push(format!("{}@{version}", query["package"]["name"].as_str().unwrap_or("")));
                results.push(if affected {
                    serde_json::json!({ "vulns": [ { "id": "RUSTSEC-2022-0013" } ] })
                } else {
                    serde_json::json!({})
                });
            }
            Ok(serde_json::json!({ "results": results }).to_string())
        });
        (found, queried)
    }

    #[test]
    fn workspace_member_resolves_inherited_regex_from_root_lock_and_osv_gets_exact_version() {
        let tmp = TempDir::new().expect("tempdir");
        cargo_workspace(tmp.path(), &[("regex", "1.11.1")]);

        let root_specs = parse_manifest(tmp.path(), Path::new("Cargo.toml")).expect("root");
        let member_specs =
            parse_manifest(tmp.path(), Path::new("crates/app/Cargo.toml")).expect("member");
        for specs in [&root_specs, &member_specs] {
            assert_eq!(specs.len(), 1);
            assert_eq!(specs[0].name, "regex");
            assert_eq!(specs[0].resolved_version.as_deref(), Some("1.11.1"));
            assert_eq!(specs[0].lockfile.as_deref(), Some("Cargo.lock"));
        }
        assert_eq!(member_specs[0].manifest_path, "crates/app/Cargo.toml");

        let (found, queried) = fake_osv(&root_specs);
        assert_eq!(queried, vec!["regex@1.11.1"]);
        assert!(found.is_empty(), "advisory for <1.5.5 must not apply to 1.11.1: {found:?}");
    }

    #[test]
    fn unresolved_requirement_still_hits_the_old_advisory() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "Cargo.toml", "[dependencies]\nregex = \"1\"\n");

        let specs = parse_manifest(tmp.path(), Path::new("Cargo.toml")).expect("manifest");
        assert_eq!(specs[0].resolved_version, None);
        assert_eq!(specs[0].lockfile, None);
        let (found, queried) = fake_osv(&specs);
        assert_eq!(queried, vec!["regex@1"]);
        assert_eq!(found.len(), 1);
    }

    #[test]
    fn two_locked_versions_of_one_crate_are_both_audited() {
        let tmp = TempDir::new().expect("tempdir");
        cargo_workspace(tmp.path(), &[("regex", "1.4.0"), ("regex", "1.11.1")]);

        let specs = parse_manifest(tmp.path(), Path::new("Cargo.toml")).expect("root");
        let versions: Vec<_> = specs.iter().filter_map(|s| s.resolved_version.as_deref()).collect();
        assert_eq!(versions, vec!["1.4.0", "1.11.1"]);

        let (found, queried) = fake_osv(&specs);
        assert_eq!(queried, vec!["regex@1.4.0", "regex@1.11.1"]);
        assert_eq!(found.keys().copied().collect::<Vec<_>>(), vec![0]);
    }

    #[test]
    fn inherited_dep_missing_from_lock_is_not_invented() {
        let tmp = TempDir::new().expect("tempdir");
        cargo_workspace(tmp.path(), &[("serde", "1.0.200")]);
        let member =
            parse_manifest(tmp.path(), Path::new("crates/app/Cargo.toml")).expect("member");
        assert!(member.is_empty());
    }

    #[test]
    fn npm_package_lock_v3_pins_dependencies_and_workspace_members_walk_up() {
        let tmp = TempDir::new().expect("tempdir");
        write(tmp.path(), "package.json", r#"{ "workspaces": ["apps/*"] }"#);
        write(
            tmp.path(),
            "apps/web/package.json",
            r#"{ "dependencies": { "lodash": "^4.17.0" } }"#,
        );
        write(
            tmp.path(),
            "package-lock.json",
            r#"{ "lockfileVersion": 3, "packages": {
                 "": {},
                 "node_modules/lodash": { "version": "4.17.21" },
                 "apps/web": {} } }"#,
        );
        let specs = parse_manifest(tmp.path(), Path::new("apps/web/package.json")).expect("npm");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].resolved_version.as_deref(), Some("4.17.21"));
        assert_eq!(specs[0].lockfile.as_deref(), Some("package-lock.json"));
        assert_eq!(specs[0].version_req.as_deref(), Some("^4.17.0"));
    }

    #[test]
    fn composer_lock_pins_dependencies() {
        let tmp = TempDir::new().expect("tempdir");
        write(
            tmp.path(),
            "composer.json",
            r#"{ "require": { "monolog/monolog": "^3.0", "php": ">=8.1" } }"#,
        );
        write(
            tmp.path(),
            "composer.lock",
            r#"{ "packages": [ { "name": "monolog/monolog", "version": "3.5.0" } ] }"#,
        );
        let specs = parse_manifest(tmp.path(), Path::new("composer.json")).expect("composer");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].resolved_version.as_deref(), Some("3.5.0"));
        assert_eq!(specs[0].lockfile.as_deref(), Some("composer.lock"));
    }

    #[test]
    fn lockfile_outside_the_repo_root_is_never_used() {
        let tmp = TempDir::new().expect("tempdir");
        write(
            tmp.path(),
            "Cargo.lock",
            "[[package]]\nname = \"regex\"\nversion = \"1.11.1\"\nsource = \"registry+x\"\n",
        );
        let repo = tmp.path().join("repo");
        write(&repo, "Cargo.toml", "[dependencies]\nregex = \"1\"\n");
        let specs = parse_manifest(&repo, Path::new("Cargo.toml")).expect("manifest");
        assert_eq!(specs[0].resolved_version, None);
    }
}
