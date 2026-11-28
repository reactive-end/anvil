//! `db.mcp`: the `anvil-db` Toolbox server across adapters, user entries, and executor grants.

use std::fs;
use std::path::Path;

use anvil_adapters::db_mcp::{DB_SERVER, executor_tool};
use anvil_adapters::sync::{init_repo_with_adapter, sync_repo};
use anvil_core::config::orchestrator::OrchestratorConfig;
use anvil_core::config::{AdapterTarget, AnvilConfig};
use serde_json::{Value, json};
use tempfile::tempdir;

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).expect("read")).expect("json")
}

fn configure(root: &Path, mcp: bool) {
    let mut config = AnvilConfig::load(root).expect("config");
    config.db.mcp = mcp;
    config.db.toolbox = ".tools/toolbox.exe".to_owned();
    config.save(root).expect("save");
}

fn seed_user_servers(path: &Path, extra: Value) {
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    let mut servers = json!({ "postgres": { "command": "pg-mcp" } });
    if let (Some(map), Some(more)) = (servers.as_object_mut(), extra.as_object()) {
        map.extend(more.clone());
    }
    fs::write(path, json!({ "mcpServers": servers }).to_string()).expect("seed");
}

#[test]
fn sync_registers_the_toolbox_in_every_adapter_and_removes_it_when_off() {
    let dir = tempdir().expect("tmp");
    let root = dir.path();
    seed_user_servers(&root.join(".mcp.json"), json!({}));
    seed_user_servers(&root.join(".agents").join("mcp_config.json"), json!({}));
    init_repo_with_adapter(root, Some(AdapterTarget::All)).expect("init");
    configure(root, true);
    sync_repo(root).expect("sync on");

    for path in [root.join(".mcp.json"), root.join(".agents").join("mcp_config.json")] {
        let servers = read_json(&path)["mcpServers"].clone();
        assert!(servers.get("postgres").is_some(), "user entry kept in {}", path.display());
        assert_eq!(
            servers[DB_SERVER],
            json!({
                "command": "anvil",
                "args": ["db", "serve"]
            })
        );
    }
    let before = fs::read_to_string(root.join(".mcp.json")).expect("read");
    sync_repo(root).expect("second sync");
    assert_eq!(before, fs::read_to_string(root.join(".mcp.json")).expect("read"), "idempotent");

    configure(root, false);
    sync_repo(root).expect("sync off");
    let servers = read_json(&root.join(".mcp.json"))["mcpServers"].clone();
    assert!(servers.get(DB_SERVER).is_none());
    assert!(servers.get("postgres").is_some());
    assert!(servers.get("anvil").is_some());
}

#[test]
fn a_user_defined_anvil_db_server_is_never_overwritten_or_removed() {
    let dir = tempdir().expect("tmp");
    let root = dir.path();
    let user = json!({ "command": "toolbox", "args": ["--prebuilt", "postgres", "--stdio"] });
    seed_user_servers(&root.join(".mcp.json"), json!({ DB_SERVER: user }));
    init_repo_with_adapter(root, Some(AdapterTarget::Claude)).expect("init");
    configure(root, true);
    sync_repo(root).expect("sync on");
    assert_eq!(read_json(&root.join(".mcp.json"))["mcpServers"][DB_SERVER], user);
    configure(root, false);
    sync_repo(root).expect("sync off");
    assert_eq!(read_json(&root.join(".mcp.json"))["mcpServers"][DB_SERVER], user);
}

#[test]
fn executors_get_the_database_tools_while_the_mcp_is_on() {
    let dir = tempdir().expect("tmp");
    let root = dir.path();
    init_repo_with_adapter(root, Some(AdapterTarget::Claude)).expect("init");
    let mut config = AnvilConfig::load(root).expect("config");
    config.orchestrator = Some(OrchestratorConfig {
        executor_tools: vec!["mcp__jira__*".to_owned()],
        ..OrchestratorConfig::default()
    });
    config.db.mcp = true;
    config.save(root).expect("save");
    sync_repo(root).expect("sync on");

    let tools = AnvilConfig::load(root).expect("config").orchestrator.expect("pair").executor_tools;
    assert_eq!(tools, vec!["mcp__jira__*".to_owned(), executor_tool()]);
    let agent = fs::read_to_string(root.join(".claude").join("agents").join("anvil-executor.md"))
        .expect("executor agent");
    assert!(agent.contains("mcp__anvil-db__*"), "{agent}");

    configure(root, false);
    sync_repo(root).expect("sync off");
    let tools = AnvilConfig::load(root).expect("config").orchestrator.expect("pair").executor_tools;
    assert_eq!(tools, vec!["mcp__jira__*".to_owned()]);
}

#[test]
fn the_codex_config_gets_the_section_too() {
    let dir = tempdir().expect("tmp");
    let root = dir.path();
    init_repo_with_adapter(root, Some(AdapterTarget::Codex)).expect("init");
    configure(root, true);
    sync_repo(root).expect("sync on");
    let toml = fs::read_to_string(root.join(".codex").join("config.toml")).expect("codex config");
    assert!(toml.contains("[mcp_servers.anvil-db]"), "{toml}");
    assert!(toml.contains("\"serve\""), "{toml}");
    configure(root, false);
    sync_repo(root).expect("sync off");
    let toml = fs::read_to_string(root.join(".codex").join("config.toml")).expect("codex config");
    assert!(!toml.contains("anvil-db"), "{toml}");
}
