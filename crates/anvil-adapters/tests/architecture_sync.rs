//! `anvil sync` states the configured architecture in the managed block of every instruction file.

use std::fs;
use std::path::Path;

use tempfile::tempdir;

use anvil_adapters::sync;
use anvil_core::config::architecture::ArchitectureStyle;
use anvil_core::config::{AdapterTarget, AnvilConfig};

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("read {rel}: {e}"))
}

fn set_style(root: &Path, style: ArchitectureStyle) {
    let mut config = AnvilConfig::load(root).expect("load config");
    config.set_architecture(style, None, None).expect("set style");
    config.save(root).expect("save config");
}

#[test]
fn sync_writes_the_architecture_line_into_claude_md_and_keeps_user_text() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    fs::write(root.join("package.json"), r#"{"name":"orders-api"}"#).expect("package.json");
    fs::write(root.join("CLAUDE.md"), "# My notes\nKeep this.\n").expect("CLAUDE.md");
    sync::init_repo_with_adapter(root, Some(AdapterTarget::Claude)).expect("init");
    assert!(!read(root, "CLAUDE.md").contains("Architecture:"), "nothing configured yet");

    set_style(root, ArchitectureStyle::Hexagonal);
    sync::sync_repo(root).expect("sync");

    let doc = read(root, "CLAUDE.md");
    assert!(doc.starts_with("# My notes\nKeep this.\n"), "user text must be preserved");
    let line = doc
        .lines()
        .find(|l| l.contains("Architecture: hexagonal"))
        .unwrap_or_else(|| panic!("architecture line missing in:\n{doc}"));
    assert!(line.contains("(ports & adapters) in this repository"), "{line}");
    assert!(line.contains("domain imports no framework or other layer"), "{line}");
    assert!(line.contains("adapters live in infrastructure"), "{line}");
    assert!(line.contains("wire them in *.module.ts"), "{line}");
    let start = doc.find("<!-- anvil:managed:start -->").expect("start");
    let end = doc.find("<!-- anvil:managed:end -->").expect("end");
    assert!(
        start < doc.find("Architecture: hexagonal").expect("line")
            && doc.find("Architecture: hexagonal").expect("line") < end
    );

    let again = {
        sync::sync_repo(root).expect("sync twice");
        read(root, "CLAUDE.md")
    };
    assert_eq!(again, doc, "sync is idempotent");

    set_style(root, ArchitectureStyle::None);
    sync::sync_repo(root).expect("sync after clearing");
    let cleared = read(root, "CLAUDE.md");
    assert!(!cleared.contains("Architecture:"), "clearing the style removes the line");
    assert!(cleared.contains("Keep this."));
}

#[test]
fn every_adapter_file_gets_the_line_generated_from_config() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    fs::write(root.join("package.json"), r#"{"name":"orders-api"}"#).expect("package.json");
    fs::create_dir_all(root.join(".cursor")).expect("cursor dir");
    fs::create_dir_all(root.join(".windsurf")).expect("windsurf dir");
    fs::create_dir_all(root.join(".codex")).expect("codex dir");
    sync::init_repo_with_adapter(root, Some(AdapterTarget::All)).expect("init");
    set_style(root, ArchitectureStyle::EventDriven);

    sync::sync_repo(root).expect("sync");

    for rel in ["CLAUDE.md", "AGENTS.md", ".cursor/rules/anvil.mdc", ".windsurf/rules/anvil.md"] {
        let doc = read(root, rel);
        assert!(
            doc.contains("Architecture: event-driven (events & handlers) in this repository: publish and consume through the event bus; handlers are idempotent; no direct calls into another context."),
            "{rel} lacks the architecture line:\n{doc}"
        );
    }
}

#[test]
fn a_legacy_root_plus_subrepo_config_gets_one_line_per_style() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();
    for name in ["orders-api", "orders-web"] {
        fs::create_dir_all(root.join(name)).expect("subrepo dir");
        fs::write(root.join(name).join("package.json"), r#"{"name":"x"}"#).expect("package.json");
    }
    sync::init_repo_with_adapter(root, Some(AdapterTarget::Claude)).expect("init");
    let mut config = AnvilConfig::load(root).expect("load config");
    assert_eq!(config.subrepos.len(), 2, "both subrepos are detected");
    config
        .set_architecture(ArchitectureStyle::Hexagonal, None, Some("orders-api/src"))
        .expect("root");
    for name in ["orders-api", "orders-web"] {
        config.set_architecture(ArchitectureStyle::Hexagonal, Some(name), None).expect("sub");
    }
    config.save(root).expect("save config");

    sync::sync_repo(root).expect("sync");

    let doc = read(root, "CLAUDE.md");
    assert_eq!(doc.matches("Architecture: hexagonal").count(), 1, "{doc}");
    assert!(doc.contains("in this repository, orders-api, orders-web:"), "{doc}");
}
