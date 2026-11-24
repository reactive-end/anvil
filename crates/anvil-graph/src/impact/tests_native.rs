//! Kotlin/Gradle and Rust targeted test runners for `impact/tests_runner.rs`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::Result;
use anvil_core::compressor::compress_output;
use anvil_core::config::AnvilConfig;
use anvil_core::fs_util::{resolve_local_or_path_bin, resolve_php_bin};
use anvil_core::proc_util::run_command_with_timeout;
use anvil_core::traits::CompressedOutput;
use rustc_hash::FxHashSet;

use super::tests_runner::{exec_compressed, skip_output};
use super::typecheck::resolve_gradle_bin;
use super::typecheck_php::build_php_tool_command;
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

pub(super) fn run_php_tests(
    repo_root: &Path,
    config: &AnvilConfig,
    files: &[&str],
) -> Result<Vec<CompressedOutput>> {
    let mut grouped: BTreeMap<PathBuf, Vec<String>> = BTreeMap::new();
    for raw in files {
        let norm = raw.replace('\\', "/");
        let work_dir = find_php_project_root(repo_root, &norm);
        let rel = repo_root
            .join(&norm)
            .strip_prefix(&work_dir)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or(norm);
        grouped.entry(work_dir).or_default().push(rel);
    }

    let mut out = Vec::with_capacity(grouped.len());
    for (work_dir, scoped_files) in grouped {
        out.push(run_php_tests_in_dir(repo_root, &work_dir, config, &scoped_files)?);
    }
    Ok(out)
}

fn find_php_project_root(repo_root: &Path, rel_file: &str) -> PathBuf {
    let abs = repo_root.join(rel_file);
    let mut cur = abs.parent();
    while let Some(dir) = cur {
        if dir.join("composer.json").is_file() || dir.join("phpunit.xml").is_file() {
            return dir.to_path_buf();
        }
        if dir == repo_root {
            break;
        }
        cur = dir.parent().filter(|p| p.starts_with(repo_root));
    }
    repo_root.to_path_buf()
}

fn run_php_tests_in_dir(
    log_root: &Path,
    work_dir: &Path,
    config: &AnvilConfig,
    files: &[String],
) -> Result<CompressedOutput> {
    if let Some(mut cmd) = build_php_tool_command(work_dir, "pest") {
        cmd.args(files);
        return exec_php_command(log_root, config, &mut cmd, "pest");
    }
    if let Some(mut cmd) = build_php_tool_command(work_dir, "phpunit") {
        cmd.args(files);
        return exec_php_command(log_root, config, &mut cmd, "phpunit");
    }
    if work_dir.join("artisan").is_file()
        && let Some(php_bin) = resolve_php_bin()
    {
        let mut cmd = Command::new(php_bin);
        cmd.arg("artisan").arg("test").args(files).current_dir(work_dir);
        return exec_php_command(log_root, config, &mut cmd, "artisan test");
    }
    Ok(skip_output("pest/phpunit", "no pest, phpunit, or php artisan runtime found"))
}

fn exec_php_command(
    log_root: &Path,
    config: &AnvilConfig,
    cmd: &mut Command,
    label: &str,
) -> Result<CompressedOutput> {
    let timeout = Duration::from_secs(config.tier_budgets.tier1_secs.max(5));
    let timed = match run_command_with_timeout(cmd, timeout) {
        Ok(out) => out,
        Err(err) => {
            return compress_output(
                Some(log_root),
                label,
                1,
                &format!("failed to execute `{label}`: {err}"),
                config.tier_budgets.compressor_max_lines,
                config.tier_budgets.compressor_max_tokens,
            );
        }
    };
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&timed.stdout),
        String::from_utf8_lossy(&timed.stderr)
    );
    compress_output(
        Some(log_root),
        label,
        timed.status_code,
        &combined,
        config.tier_budgets.compressor_max_lines,
        config.tier_budgets.compressor_max_tokens,
    )
}
