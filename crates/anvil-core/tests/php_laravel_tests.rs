use std::fs;
use std::path::Path;

use anyhow::Result;
use tempfile::TempDir;

use anvil_core::compressor::compress_output;
use anvil_core::config::{AnvilConfig, StackKind, ToolKind};
use anvil_core::diff_classifier::classify_paths;
use anvil_core::diff_grep::is_noise_file;
use anvil_core::fs_util::{resolve_local_or_path_bin, resolve_php_vendor_script};
use anvil_core::stack_detector::detect_workspace;
use anvil_core::traits::{DiffCategory, Tier};

fn write_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

#[test]
fn detects_laravel_subrepo_and_frontend_monorepo() -> Result<()> {
    let tmp = TempDir::new()?;
    let root = tmp.path();

    write_file(
        &root.join("backend/composer.json"),
        r#"{
          "name": "acme/backend",
          "require": { "php": "^8.2", "laravel/framework": "^12.0" },
          "require-dev": {
            "larastan/larastan": "^3.13",
            "laravel/pint": "^1.24",
            "phpunit/phpunit": "^11.5",
            "pestphp/pest": "^3.0"
          }
        }"#,
    )?;
    write_file(&root.join("backend/artisan"), "#!/usr/bin/env php\n")?;
    write_file(&root.join("backend/phpstan.neon"), "parameters:\n  level: 6\n")?;
    write_file(&root.join("backend/vendor/ignored/composer.json"), "{}")?;

    write_file(
        &root.join("frontend/package.json"),
        r#"{"dependencies":{"vue":"^3.5.0"},"devDependencies":{"vite":"^8.0.0","vitest":"^5.0.0","eslint":"^9.0.0"}}"#,
    )?;
    write_file(&root.join("frontend/tsconfig.json"), r#"{"compilerOptions":{"strict":true}}"#)?;
    write_file(&root.join("frontend/vite.config.ts"), "")?;

    let det = detect_workspace(root)?;
    assert!(det.is_multi_repo);
    assert_eq!(det.repos.len(), 2, "vendor/ must be ignored: {:?}", det.repos);

    let backend = det.repos.iter().find(|r| r.rel_path == "backend").expect("backend subrepo");
    assert!(backend.stacks.contains(&StackKind::Php));
    assert!(backend.stacks.contains(&StackKind::Laravel));
    assert!(backend.tools.contains(&ToolKind::Composer));
    assert!(backend.tools.contains(&ToolKind::PhpStan));
    assert!(backend.tools.contains(&ToolKind::Pint));
    assert!(backend.tools.contains(&ToolKind::PhpUnit));
    assert!(backend.tools.contains(&ToolKind::Pest));

    let mut cfg = AnvilConfig::default();
    det.apply_to_config(&mut cfg);
    assert!(cfg.root_allowlist.contains(&"artisan".to_owned()));
    assert!(cfg.root_allowlist.contains(&"composer.json".to_owned()));
    assert!(cfg.root_allowlist.contains(&"routes".to_owned()));
    assert!(cfg.root_allowlist.contains(&"resources".to_owned()));
    Ok(())
}

#[test]
fn detects_php_noise_files() {
    assert!(is_noise_file("composer.lock"));
    assert!(is_noise_file("backend/composer.lock"));
    assert!(is_noise_file("backend/vendor/laravel/framework/src/Foo.php"));
    assert!(is_noise_file("backend/storage/framework/views/abc.php"));
    assert!(is_noise_file("backend/.phpunit.result.cache"));
    assert!(!is_noise_file("backend/app/Services/OrderService.php"));
}

#[test]
fn classifies_php_laravel_paths() {
    let deps = classify_paths(&["backend/composer.json"]);
    assert_eq!(
        (deps.primary_category, deps.minimum_tier, deps.needs_deps_audit),
        (DiffCategory::Deps, Tier::Tier2, true)
    );

    let blade = classify_paths(&["backend/resources/views/users/index.blade.php"]);
    assert_eq!(
        (blade.primary_category, blade.minimum_tier, blade.needs_visual_qa),
        (DiffCategory::Ui, Tier::Tier2, true)
    );

    let routes = classify_paths(&["backend/routes/api/billing.php"]);
    assert_eq!(
        (routes.primary_category, routes.minimum_tier),
        (DiffCategory::Contract, Tier::Tier1)
    );

    let cfg = classify_paths(&["backend/phpstan.neon"]);
    assert_eq!((cfg.primary_category, cfg.minimum_tier), (DiffCategory::Config, Tier::Tier1));
}

#[test]
fn php_output_compressor_handles_phpstan_phpunit_and_syntax_errors() -> Result<()> {
    let pass = compress_output(None, "phpstan analyse", 0, "[OK] No errors\n", 50, 1200)?;
    assert!(pass.passed);
    assert_eq!(pass.tool, "php");

    let phpstan_raw = "app/Services/BillingService.php:42:Parameter #1 $amount of method charge() expects int, string given.\n";
    let fail = compress_output(None, "phpstan analyse", 1, phpstan_raw, 50, 1200)?;
    assert!(!fail.passed);
    assert_eq!(fail.tool, "php");
    assert_eq!(fail.total_errors, 1);
    assert!(fail.lines[0].contains("app/Services/BillingService.php:42:"));

    let syntax_raw =
        "Parse error: syntax error, unexpected token \"}\" in app/Models/User.php on line 18\n";
    let syn_fail = compress_output(None, "php -l app/Models/User.php", 255, syntax_raw, 50, 1200)?;
    assert!(!syn_fail.passed);
    assert!(syn_fail.lines[0].contains("app/Models/User.php:18: Parse error:"));

    let phpunit_raw = "FAIL  Tests\\Feature\\Auth\\LoginTest\n⨯ it rejects invalid password\nFailed asserting that 200 matches expected 401.\nTests: 1 failed (1 assertions)\n";
    let unit_fail = compress_output(None, "vendor/bin/phpunit", 1, phpunit_raw, 50, 1200)?;
    assert!(!unit_fail.passed);
    assert!(unit_fail.lines.iter().any(|l| l.contains("Failed asserting")));
    Ok(())
}

#[test]
fn resolves_vendor_bin_scripts() -> Result<()> {
    let tmp = TempDir::new()?;
    let script = tmp.path().join("vendor/bin/phpstan");
    write_file(&script, "#!/usr/bin/env php\n<?php\n")?;
    #[cfg(windows)]
    write_file(&tmp.path().join("vendor/bin/phpstan.bat"), "@echo off\r\n")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&script)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script, perms)?;
    }

    assert_eq!(resolve_php_vendor_script(tmp.path(), "phpstan"), Some(script));
    assert!(resolve_local_or_path_bin(tmp.path(), "phpstan").is_some());
    Ok(())
}
