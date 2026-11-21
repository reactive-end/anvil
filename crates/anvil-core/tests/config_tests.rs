use anyhow::Result;
use tempfile::TempDir;

use anvil_core::config::{
    AnvilConfig, MobileQaDriver, QaDriverConfig, StackKind, ToolKind, WebQaDriver,
};
use anvil_core::traits::Language;

#[test]
fn config_defaults_match_spec() {
    let cfg = AnvilConfig::default();
    assert_eq!(cfg.version, 1);
    assert_eq!(cfg.circuit_breaker_limit, 3);
    assert_eq!(cfg.deps_max_age_months, 24);
    assert_eq!(cfg.tier_budgets.tier0_ms, 500);
    assert_eq!(cfg.tier_budgets.tier1_secs, 15);
    assert_eq!(cfg.tier_budgets.compressor_max_lines, 50);
    assert_eq!(cfg.tier_budgets.compressor_max_tokens, 1200);
    assert!(cfg.root_allowlist.iter().any(|s| s == "tasks.json"));
    assert!(cfg.allowed_escapes.iter().any(|s| s == "anvil-allow"));
}

#[test]
fn config_roundtrip_save_and_load() -> Result<()> {
    let tmp = TempDir::new()?;
    let cfg = AnvilConfig {
        stacks: vec![StackKind::React, StackKind::NestJs],
        tools: vec![ToolKind::Eslint, ToolKind::Vitest],
        qa_driver: QaDriverConfig {
            web: WebQaDriver::Playwright,
            mobile: MobileQaDriver::Roborazzi,
        },
        ..Default::default()
    };

    cfg.save(tmp.path())?;
    assert!(AnvilConfig::path(tmp.path()).exists());

    let loaded = AnvilConfig::load(tmp.path())?;
    assert_eq!(loaded, cfg);
    Ok(())
}

#[test]
fn config_load_or_default_handles_missing_and_invalid() -> Result<()> {
    let tmp = TempDir::new()?;
    let fallback = AnvilConfig::load_or_default(tmp.path())?;
    assert_eq!(fallback, AnvilConfig::default());

    let cfg_path = AnvilConfig::path(tmp.path());
    if let Some(parent) = cfg_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&cfg_path, "{ not valid json")?;
    assert!(AnvilConfig::load_or_default(tmp.path()).is_err());
    Ok(())
}

#[test]
fn language_rust_serializes_as_snake_case() -> Result<()> {
    let json = serde_json::to_string(&Language::Rust)?;
    assert_eq!(json, "\"rust\"");
    let decoded: Language = serde_json::from_str(&json)?;
    assert_eq!(decoded, Language::Rust);
    Ok(())
}
