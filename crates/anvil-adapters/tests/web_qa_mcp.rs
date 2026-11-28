//! `chrome_dev_tools` driver: MCP entry registration, removal, and executor grants across adapters.

use std::fs;
use std::path::Path;

use anvil_adapters::sync::{init_repo_with_adapter, sync_repo};
use anvil_adapters::web_qa::{CHROME_DEVTOOLS_MCP_VERSION, executor_tool};
use anvil_core::config::orchestrator::OrchestratorConfig;
use anvil_core::config::{AdapterTarget, AnvilConfig, WebQaDriver};
use serde_json::{Value, json};
use tempfile::tempdir;

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).expect("read")).expect("json")
}

fn set_driver(root: &Path, driver: WebQaDriver) {
    let mut config = AnvilConfig::load(root).expect("config");
    config.qa_driver.web = driver;
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
fn sync_registers_the_server_in_every_adapter_and_keeps_user_entries() {
    let dir = tempdir().expect("tmp");
    let root = dir.path();
    seed_user_servers(&root.join(".mcp.json"), json!({}));
    seed_user_servers(&root.join(".agents").join("mcp_config.json"), json!({}));
    init_repo_with_adapter(root, Some(AdapterTarget::All)).expect("init");
    set_driver(root, WebQaDriver::ChromeDevTools);
    sync_repo(root).expect("sync");

    for path in [root.join(".mcp.json"), root.join(".agents").join("mcp_config.json")] {
        let servers = read_json(&path)["mcpServers"].clone();
        assert!(servers.get("postgres").is_some(), "user entry kept in {}", path.display());
        assert!(servers.get("anvil").is_some(), "anvil entry kept in {}", path.display());
        let entry = &servers["chrome-devtools"];
        let args: Vec<&str> =
            entry["args"].as_array().expect("args").iter().filter_map(Value::as_str).collect();
        let pinned = format!("chrome-devtools-mcp@{CHROME_DEVTOOLS_MCP_VERSION}");
        assert!(args.contains(&pinned.as_str()), "{args:?}");
        for flag in ["--isolated", "--headless", "--no-usage-statistics", "--no-performance-crux"] {
            assert!(args.contains(&flag), "{flag} missing in {args:?}");
        }
    }

    let before = fs::read_to_string(root.join(".mcp.json")).expect("read");
    sync_repo(root).expect("second sync");
    assert_eq!(before, fs::read_to_string(root.join(".mcp.json")).expect("read"), "idempotent");
}

#[test]
fn changing_the_driver_removes_only_the_managed_entry() {
    let dir = tempdir().expect("tmp");
    let root = dir.path();
    seed_user_servers(&root.join(".mcp.json"), json!({}));
    init_repo_with_adapter(root, Some(AdapterTarget::Claude)).expect("init");
    set_driver(root, WebQaDriver::ChromeDevTools);
    sync_repo(root).expect("sync on");
    assert!(read_json(&root.join(".mcp.json"))["mcpServers"].get("chrome-devtools").is_some());

    set_driver(root, WebQaDriver::Playwright);
    sync_repo(root).expect("sync off");
    let servers = read_json(&root.join(".mcp.json"))["mcpServers"].clone();
    assert!(servers.get("chrome-devtools").is_none());
    assert!(servers.get("postgres").is_some());
    assert!(servers.get("anvil").is_some());
}

#[test]
fn a_user_defined_chrome_devtools_server_is_never_overwritten_or_removed() {
    let dir = tempdir().expect("tmp");
    let root = dir.path();
    let user = json!({ "command": "npx", "args": ["chrome-devtools-mcp@latest", "--browser-url=http://127.0.0.1:9222"] });
    seed_user_servers(&root.join(".mcp.json"), json!({ "chrome-devtools": user }));
    init_repo_with_adapter(root, Some(AdapterTarget::Claude)).expect("init");
    set_driver(root, WebQaDriver::ChromeDevTools);
    sync_repo(root).expect("sync on");
    assert_eq!(read_json(&root.join(".mcp.json"))["mcpServers"]["chrome-devtools"], user);
    set_driver(root, WebQaDriver::Skip);
    sync_repo(root).expect("sync off");
    assert_eq!(read_json(&root.join(".mcp.json"))["mcpServers"]["chrome-devtools"], user);
}

#[test]
fn executors_get_the_tools_while_the_driver_is_chrome_dev_tools() {
    let dir = tempdir().expect("tmp");
    let root = dir.path();
    init_repo_with_adapter(root, Some(AdapterTarget::Claude)).expect("init");
    let mut config = AnvilConfig::load(root).expect("config");
    config.orchestrator = Some(OrchestratorConfig {
        executor_tools: vec!["mcp__jira__*".to_owned()],
        ..OrchestratorConfig::default()
    });
    config.qa_driver.web = WebQaDriver::ChromeDevTools;
    config.save(root).expect("save");
    sync_repo(root).expect("sync on");

    let tools = AnvilConfig::load(root).expect("config").orchestrator.expect("pair").executor_tools;
    assert_eq!(tools, vec!["mcp__jira__*".to_owned(), executor_tool()]);
    let agent = fs::read_to_string(root.join(".claude").join("agents").join("anvil-executor.md"))
        .expect("executor agent");
    assert!(agent.contains("mcp__chrome-devtools__*"), "{agent}");

    set_driver(root, WebQaDriver::Skip);
    sync_repo(root).expect("sync off");
    let tools = AnvilConfig::load(root).expect("config").orchestrator.expect("pair").executor_tools;
    assert_eq!(tools, vec!["mcp__jira__*".to_owned()]);
    let agent = fs::read_to_string(root.join(".claude").join("agents").join("anvil-executor.md"))
        .expect("executor agent");
    assert!(!agent.contains("chrome-devtools"), "{agent}");
}
