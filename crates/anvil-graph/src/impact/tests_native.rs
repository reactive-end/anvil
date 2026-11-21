//! Kotlin/Gradle and Rust targeted test runners for `impact/tests_runner.rs`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use anvil_core::config::AnvilConfig;
use anvil_core::fs_util::resolve_local_or_path_bin;
use anvil_core::traits::CompressedOutput;
use rustc_hash::FxHashSet;

use super::tests_runner::{exec_compressed, skip_output};
use super::typecheck::resolve_gradle_bin;
use crate::resolvers::kotlin::extract_kotlin_package;
use crate::resolvers::rust_cargo::parse_package_name;

pub(super) fn run_kotlin_tests(
    repo_root: &Path,
    config: &AnvilConfig,
    files: &[&str],
) -> Result<CompressedOutput> {
    let Some((work_dir, gradle)) = find_gradle_root(repo_root, files[0]) else {
        return Ok(skip_output("gradlew", "gradle/gradlew binary not found"));
    };
    let mut owned_args = vec!["testDebugUnitTest".to_owned()];
    for f in files {
        owned_args.push("--tests".to_owned());
        owned_args.push(build_kotlin_test_filter(repo_root, f));
    }
    let arg_refs: Vec<&str> = owned_args.iter().map(String::as_str).collect();
    exec_compressed(
        repo_root,
        &work_dir,
        config,
        &gradle,
        "gradlew testDebugUnitTest",
        &arg_refs,
    )
}

fn find_gradle_root(repo_root: &Path, sample_rel: &str) -> Option<(PathBuf, PathBuf)> {
    let abs = repo_root.join(sample_rel.replace('\\', "/"));
    let mut cur = abs.parent();
    while let Some(dir) = cur {
        if let Some(bin) = resolve_gradle_bin(dir) {
            return Some((dir.to_path_buf(), bin));
        }
        if dir == repo_root {
            break;
        }
        cur = dir.parent().filter(|p| p.starts_with(repo_root));
    }
    resolve_gradle_bin(repo_root).map(|bin| (repo_root.to_path_buf(), bin))
}

fn build_kotlin_test_filter(repo_root: &Path, rel_file: &str) -> String {
    let norm = rel_file.replace('\\', "/");
    let path = repo_root.join(&norm);
    let cls = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if let Ok(content) = fs::read_to_string(&path)
        && let Some(pkg) = extract_kotlin_package(&content)
        && !cls.is_empty()
    {
        return format!("{pkg}.{cls}");
    }
    if !cls.is_empty() { format!("*{cls}") } else { norm }
}

pub(super) fn run_rust_tests(
    repo_root: &Path,
    config: &AnvilConfig,
    files: &[&str],
) -> Result<Vec<CompressedOutput>> {
    let Some(cargo) = resolve_local_or_path_bin(repo_root, "cargo") else {
        return Ok(vec![skip_output("cargo", "cargo binary not found")]);
    };

    let mut crates = Vec::new();
    let mut seen = FxHashSet::default();
    for f in files {
        let pkg = detect_rust_crate_for_file(repo_root, f);
        if seen.insert(pkg.clone()) {
            crates.push(pkg);
        }
    }

    let mut out = Vec::with_capacity(crates.len());
    for pkg in crates {
        if pkg.is_empty() {
            out.push(exec_compressed(repo_root, repo_root, config, &cargo, "cargo test", &["test"])?);
        } else {
            let label = format!("cargo test -p {pkg}");
            out.push(exec_compressed(
                repo_root,
                repo_root,
                config,
                &cargo,
                &label,
                &["test", "-p", &pkg],
            )?);
        }
    }
    Ok(out)
}

fn detect_rust_crate_for_file(repo_root: &Path, rel_file: &str) -> String {
    let norm = rel_file.replace('\\', "/");
    let abs = repo_root.join(&norm);
    let mut cur = abs.parent();
    while let Some(dir) = cur {
        let toml_path = dir.join("Cargo.toml");
        if toml_path.is_file()
            && let Ok(content) = fs::read_to_string(&toml_path)
            && let Some(pkg) = parse_package_name(&content)
        {
            return pkg;
        }
        if dir == repo_root {
            break;
        }
        cur = dir.parent().filter(|p| p.starts_with(repo_root));
    }
    String::new()
}
