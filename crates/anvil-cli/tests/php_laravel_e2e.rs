//! End-to-end CLI tests for PHP 8.x & Laravel workspaces (init, doctor, check Tier 0, graph, token guard).

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::json;
use tempfile::TempDir;

fn anvil_bin() -> &'static str {
    env!("CARGO_BIN_EXE_anvil")
}

fn run_anvil(repo: &Path, args: &[&str]) -> Output {
    Command::new(anvil_bin())
        .arg("--root")
        .arg(repo)
        .args(args)
        .output()
        .expect("execute anvil")
}

fn run_anvil_stdin(repo: &Path, args: &[&str], stdin_data: &str) -> Output {
    let mut child = Command::new(anvil_bin())
        .arg("--root")
        .arg(repo)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn anvil");
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(stdin_data.as_bytes()).unwrap();
    }
    child.wait_with_output().unwrap()
}

fn write_file(path: &Path, content: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, content).unwrap();
}

fn setup_laravel_git_repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    let root = dir.path();

    Command::new("git").args(["init"]).current_dir(root).output().unwrap();
    Command::new("git").args(["config", "user.name", "Laravel Dev"]).current_dir(root).output().unwrap();
    Command::new("git").args(["config", "user.email", "dev@example.com"]).current_dir(root).output().unwrap();

    write_file(
        &root.join("backend/composer.json"),
        r#"{
          "name": "acme/laravel-app",
          "require": { "php": "^8.2", "laravel/framework": "^12.0" },
          "require-dev": { "larastan/larastan": "^3.13", "laravel/pint": "^1.24", "phpunit/phpunit": "^11.5" },
          "autoload": { "psr-4": { "App\\": "app/" } }
        }"#,
    );
    write_file(&root.join("backend/composer.lock"), "{}\n");
    write_file(&root.join("backend/artisan"), "#!/usr/bin/env php\n");
    write_file(
        &root.join("backend/phpstan.neon"),
        "includes:\n  - vendor/larastan/larastan/extension.neon\nparameters:\n  level: 6\n",
    );
    write_file(
        &root.join("backend/app/Services/QuoteService.php"),
        r#"<?php

declare(strict_types=1);

namespace App\Services;

final class QuoteService
{
    public function total(int $cents): int
    {
        return $cents;
    }
}
"#,
    );

    Command::new("git").args(["add", "-A"]).current_dir(root).output().unwrap();
    Command::new("git").args(["commit", "-m", "initial"]).current_dir(root).output().unwrap();
    dir
}

#[test]
fn laravel_init_doctor_tier0_strict_types_graph_and_token_guard() {
    let repo = setup_laravel_git_repo();
    let root = repo.path();

    let init_out = run_anvil(root, &["init"]);
    assert!(init_out.status.success(), "init failed: {}", String::from_utf8_lossy(&init_out.stderr));

    let cfg_raw = fs::read_to_string(root.join(".harness/config.json")).unwrap();
    assert!(cfg_raw.contains("\"laravel\""));
    assert!(cfg_raw.contains("\"php\""));
    assert!(cfg_raw.contains("\"php_stan\""));

    let doc_out = run_anvil(root, &["doctor"]);
    let doc_stdout = String::from_utf8_lossy(&doc_out.stdout);
    assert!(doc_out.status.success(), "doctor failed: {doc_stdout}");
    assert!(doc_stdout.contains("phpstan: level=6"), "doctor output: {doc_stdout}");

    // Tier 0 blocks PHP file missing declare(strict_types=1) and using dd()
    write_file(
        &root.join("backend/app/Services/BadService.php"),
        "<?php\n\nnamespace App\\Services;\n\nfinal class BadService { public function x(): void { dd(1); } }\n",
    );
    let check_fail = run_anvil(root, &["check", "--no-tests"]);
    let fail_stdout = String::from_utf8_lossy(&check_fail.stdout);
    assert!(!check_fail.status.success(), "expected Tier 0 failure, got: {fail_stdout}");
    assert!(fail_stdout.contains("require-strict-types"), "stdout: {fail_stdout}");
    assert!(fail_stdout.contains("no-debug-helpers"), "stdout: {fail_stdout}");

    // Fixing both makes Tier 0 clean
    write_file(
        &root.join("backend/app/Services/BadService.php"),
        "<?php\n\ndeclare(strict_types=1);\n\nnamespace App\\Services;\n\nfinal class BadService { public function x(): int { return 1; } }\n",
    );
    let outline = run_anvil(root, &["graph", "outline", "backend/app/Services/QuoteService.php"]);
    let outline_stdout = String::from_utf8_lossy(&outline.stdout);
    assert!(outline.status.success(), "outline failed: {outline_stdout}");
    assert!(outline_stdout.contains("QuoteService"), "outline: {outline_stdout}");

    // Token Guard blocks unbounded read of composer.lock (Antigravity 2.0 `Path` key)
    let payload = json!({
        "hookEventName": "PreToolUse",
        "cwd": root.to_string_lossy(),
        "toolCall": {
            "toolName": "read_file",
            "input": { "Path": root.join("backend/composer.lock").to_string_lossy() }
        }
    })
    .to_string();
    let guard_out = run_anvil_stdin(root, &["hook", "PreToolUse", "--adapter", "antigravity"], &payload);
    let guard_stdout = String::from_utf8_lossy(&guard_out.stdout);
    assert!(guard_out.status.success(), "hook failed: {guard_stdout}");
    assert!(guard_stdout.contains("\"decision\":\"deny\""), "expected token guard deny for composer.lock: {guard_stdout}");
}

#[test]
fn antigravity_v2_edit_file_with_capital_path_tracks_edit_and_gates_stop() {
    let repo = setup_laravel_git_repo();
    let root = repo.path();
    assert!(run_anvil(root, &["init"]).status.success());

    let pre_inv = json!({
        "hookEventName": "PreInvocation",
        "cwd": root.to_string_lossy(),
        "lastUserInput": "Ajusta QuoteService para aplicar redondeo fiscal"
    })
    .to_string();
    assert!(run_anvil_stdin(root, &["hook", "PreInvocation", "--adapter", "antigravity"], &pre_inv).status.success());
    assert!(root.join(".harness/.state/pending-prompt.json").is_file());

    let edited = root.join("backend/app/Services/QuoteService.php");
    write_file(
        &edited,
        "<?php\n\ndeclare(strict_types=1);\n\nnamespace App\\Services;\n\nfinal class QuoteService { public function total(int $cents): int { return $cents + 1; } }\n",
    );
    write_file(
        &root.join("backend/app/Services/TaxService.php"),
        "<?php\n\ndeclare(strict_types=1);\n\nnamespace App\\Services;\n\nfinal class TaxService { public function rate(): int { return 16; } }\n",
    );

    let post_tool = json!({
        "hookEventName": "PostToolUse",
        "cwd": root.to_string_lossy(),
        "toolCall": {
            "toolName": "default_api:edit_file",
            "input": { "Path": edited.to_string_lossy(), "Target": "a", "Replacement": "b" }
        }
    })
    .to_string();
    assert!(run_anvil_stdin(root, &["hook", "PostToolUse", "--adapter", "antigravity"], &post_tool).status.success());
    assert!(!root.join(".harness/.state/active-task.json").exists(), "PostToolUse must never auto-materialize raw tasks");
    let tracked = fs::read_to_string(root.join(".harness/.state/untracked-edits.json")).unwrap_or_default();
    assert!(tracked.contains("QuoteService.php"), "expected tracked edit for capital Path: {tracked}");

    let stop_in = json!({ "hookEventName": "Stop", "cwd": root.to_string_lossy() }).to_string();
    let stop_out = run_anvil_stdin(root, &["hook", "Stop", "--adapter", "antigravity"], &stop_in);
    let stop_stdout = String::from_utf8_lossy(&stop_out.stdout);
    assert!(stop_stdout.contains("\"decision\":\"continue\""), "expected Stop gate requiring task decision: {stop_stdout}");
    assert!(stop_stdout.contains("anvil tasks record"), "stop output: {stop_stdout}");
}

#[test]
fn stop_hook_gates_substantive_external_edits_without_auto_creating_task() {
    let repo = setup_laravel_git_repo();
    let root = repo.path();
    assert!(run_anvil(root, &["init"]).status.success());

    let pre_inv = json!({
        "hookEventName": "PreInvocation",
        "cwd": root.to_string_lossy(),
        "lastUserInput": "Actualiza QuoteService via comando externo"
    })
    .to_string();
    assert!(run_anvil_stdin(root, &["hook", "PreInvocation", "--adapter", "antigravity"], &pre_inv).status.success());

    write_file(
        &root.join("backend/app/Services/QuoteService.php"),
        "<?php\n\ndeclare(strict_types=1);\n\nnamespace App\\Services;\n\nfinal class QuoteService { public function total(int $cents): int { return $cents + 2; } }\n",
    );
    write_file(
        &root.join("backend/app/Services/TaxService.php"),
        "<?php\n\ndeclare(strict_types=1);\n\nnamespace App\\Services;\n\nfinal class TaxService { public function rate(): int { return 16; } }\n",
    );
    assert!(!root.join(".harness/.state/active-task.json").exists());

    let stop_in = json!({ "hookEventName": "Stop", "cwd": root.to_string_lossy() }).to_string();
    let stop_out = run_anvil_stdin(root, &["hook", "Stop", "--adapter", "antigravity"], &stop_in);
    let stop_stdout = String::from_utf8_lossy(&stop_out.stdout);
    assert!(!root.join(".harness/.state/active-task.json").exists(), "Stop must never auto-materialize raw tasks");
    assert!(stop_stdout.contains("\"decision\":\"continue\""), "expected Stop to gate substantive edits until curated: {stop_stdout}");
    assert!(stop_stdout.contains("anvil tasks record"), "stop output: {stop_stdout}");
}

#[test]
fn cli_renders_ascii_anvil_banner_and_respects_color_controls() {
    let help = Command::new(anvil_bin()).arg("--help").output().expect("help");
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(help.status.success());
    assert!(help_text.contains("A N V I L"), "missing ASCII banner: {help_text}");
    assert!(help_text.contains("⠈⠛⠓⠓⠉⠉⠉⠁"), "missing 3D Braille anvil base: {help_text}");

    let repo = setup_laravel_git_repo();
    let root = repo.path();
    assert!(run_anvil(root, &["init"]).status.success());

    let colored = Command::new(anvil_bin())
        .env("ANVIL_COLOR", "always")
        .arg("--root")
        .arg(root)
        .arg("doctor")
        .output()
        .expect("colored doctor");
    let colored_out = String::from_utf8_lossy(&colored.stdout);
    assert!(colored.status.success());
    assert!(colored_out.contains('\x1b'), "expected ANSI color sequences when ANVIL_COLOR=always");
    assert!(colored_out.contains("A N V I L"), "expected banner on forced color doctor");

    let plain = Command::new(anvil_bin())
        .env("ANVIL_COLOR", "always")
        .arg("--no-color")
        .arg("--root")
        .arg(root)
        .arg("doctor")
        .output()
        .expect("no-color doctor");
    let plain_out = String::from_utf8_lossy(&plain.stdout);
    assert!(plain.status.success());
    assert!(!plain_out.contains('\x1b'), "expected zero ANSI escapes with --no-color: {plain_out}");
}
