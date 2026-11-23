//! Tests for Composer (`composer.json`) manifest parsing and Packagist auditing.

use std::fs;
use std::path::PathBuf;

use anyhow::Result;
use chrono::{TimeZone, Utc};
use tempfile::TempDir;

use anvil_deps::composer::{parse_composer_json, parse_packagist_json};
use anvil_deps::{
    AuditOptions, DepIssueKind, Ecosystem, PackageHealthInfo, RegistryFetcher, audit_manifests,
    is_supported_manifest,
};

struct MockPackagist;

impl RegistryFetcher for MockPackagist {
    fn fetch(&self, ecosystem: Ecosystem, package: &str) -> Result<PackageHealthInfo> {
        assert_eq!(ecosystem, Ecosystem::Composer);
        match package {
            "laravel/framework" => Ok(PackageHealthInfo {
                ecosystem,
                name: package.to_owned(),
                latest_version: Some("v12.1.0".to_owned()),
                last_publish: Some(Utc.with_ymd_and_hms(2026, 10, 1, 0, 0, 0).unwrap()),
                deprecated: false,
                deprecation_message: None,
                archived: false,
                repository_url: Some("https://github.com/laravel/framework".to_owned()),
            }),
            "swiftmailer/swiftmailer" => Ok(PackageHealthInfo {
                ecosystem,
                name: package.to_owned(),
                latest_version: Some("v6.3.0".to_owned()),
                last_publish: Some(Utc.with_ymd_and_hms(2021, 11, 15, 0, 0, 0).unwrap()),
                deprecated: true,
                deprecation_message: Some(
                    "Package is abandoned on Packagist; use `symfony/mailer` instead".to_owned(),
                ),
                archived: false,
                repository_url: None,
            }),
            other => anyhow::bail!("unexpected package {other}"),
        }
    }
}

#[test]
fn parses_composer_json_manifest() {
    assert!(is_supported_manifest(&PathBuf::from("backend/composer.json")));

    let manifest = r#"{
      "require": {
        "php": "^8.2",
        "ext-json": "*",
        "lib-openssl": "*",
        "laravel/framework": "^12.0",
        "firebase/php-jwt": "^7.2"
      },
      "require-dev": {
        "larastan/larastan": "^3.13",
        "phpunit/phpunit": "^11.5"
      }
    }"#;
    let specs = parse_composer_json("backend/composer.json", manifest).expect("composer.json");
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "firebase/php-jwt",
            "larastan/larastan",
            "laravel/framework",
            "phpunit/phpunit"
        ]
    );
    assert!(specs.iter().all(|s| s.ecosystem == Ecosystem::Composer));
}

#[test]
fn parses_packagist_active_and_abandoned_responses() {
    let packagist_active = r#"{
      "package": {
        "name": "laravel/framework",
        "repository": "https://github.com/laravel/framework",
        "abandoned": false,
        "versions": {
          "dev-master": { "version": "dev-master", "time": "2026-11-20T00:00:00+00:00" },
          "v12.0.0": { "version": "v12.0.0", "time": "2026-02-01T00:00:00+00:00" },
          "v12.4.0": { "version": "v12.4.0", "time": "2026-10-15T12:00:00+00:00" }
        }
      }
    }"#;
    let info = parse_packagist_json("laravel/framework", packagist_active).expect("active");
    assert!(!info.deprecated);
    assert_eq!(info.latest_version.as_deref(), Some("v12.4.0"));
    assert_eq!(info.repository_url.as_deref(), Some("https://github.com/laravel/framework"));

    let packagist_abandoned = r#"{
      "package": {
        "name": "swiftmailer/swiftmailer",
        "abandoned": "symfony/mailer",
        "versions": {
          "v6.3.0": { "version": "v6.3.0", "time": "2021-11-15T10:00:00+00:00" }
        }
      }
    }"#;
    let old = parse_packagist_json("swiftmailer/swiftmailer", packagist_abandoned).expect("abandoned");
    assert!(old.deprecated);
    assert!(old.deprecation_message.as_deref().unwrap_or("").contains("symfony/mailer"));
}

#[test]
fn audits_composer_manifest_and_flags_abandoned_and_stale() -> Result<()> {
    let tmp = TempDir::new()?;
    let manifest_path = tmp.path().join("backend/composer.json");
    fs::create_dir_all(manifest_path.parent().unwrap())?;
    fs::write(
        &manifest_path,
        r#"{"require":{"php":"^8.2","laravel/framework":"^12.0","swiftmailer/swiftmailer":"^6.3"}}"#,
    )?;

    let opts = AuditOptions {
        max_age_months: 24,
        cache_ttl_hours: 24,
        now: Some(Utc.with_ymd_and_hms(2026, 11, 23, 0, 0, 0).unwrap()),
    };
    let report = audit_manifests(tmp.path(), &[PathBuf::from("backend/composer.json")], &MockPackagist, &opts)?;
    assert_eq!(report.checked_packages, 2);
    assert_eq!(report.findings.len(), 1);
    assert_eq!(report.findings[0].package, "swiftmailer/swiftmailer");
    assert!(report.findings[0].issues.contains(&DepIssueKind::Deprecated));
    assert!(report.findings[0].issues.contains(&DepIssueKind::Stale));
    Ok(())
}
