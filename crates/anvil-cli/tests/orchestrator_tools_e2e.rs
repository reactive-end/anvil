use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn anvil(repo: &Path, claude_dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_anvil"))
        .arg("--root")
        .arg(repo)
        .args(args)
        .env("CLAUDE_CONFIG_DIR", claude_dir)
        .output()
        .expect("failed to execute anvil binary")
}

fn ok(out: &Output, what: &str) -> String {
    assert!(
        out.status.success(),
        "{what} failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn tools_lists_configured_servers_and_writes_the_executor_allowlist() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path().join("repo");
    let claude = tmp.path().join("claude");
    fs::create_dir_all(&repo).unwrap();
    fs::create_dir_all(&claude).unwrap();
    assert!(Command::new("git").arg("init").arg("-q").arg(&repo).status().unwrap().success());
    ok(&anvil(&repo, &claude, &["init"]), "init");
    fs::write(repo.join(".mcp.json"), r#"{"mcpServers":{"anvil":{},"postgres":{}}}"#).unwrap();
    fs::write(claude.join(".claude.json"), r#"{"mcpServers":{"jira":{}}}"#).unwrap();

    let list = ok(&anvil(&repo, &claude, &["orchestrator", "tools", "--list"]), "list");
    assert!(list.contains("[ ] postgres") && list.contains("project"), "{list}");
    assert!(list.contains("[ ] jira") && !list.contains("] anvil"), "{list}");

    let add = ["orchestrator", "tools", "--add", "postgres,chrome-devtools"];
    let added = ok(&anvil(&repo, &claude, &add), "add");
    assert!(
        added.contains("[x] postgres") && added.contains("[x] mcp__chrome-devtools__*"),
        "{added}"
    );
    let executor = repo.join(".claude/agents/anvil-executor.md");
    let tools_line = |path: &Path| {
        let text = fs::read_to_string(path).unwrap();
        text.lines().find(|l| l.starts_with("tools: ")).unwrap().to_owned()
    };
    assert!(
        tools_line(&executor).ends_with("mcp__anvil__*, mcp__postgres__*, mcp__chrome-devtools__*"),
        "{}",
        tools_line(&executor)
    );

    ok(&anvil(&repo, &claude, &["orchestrator", "tools", "--remove", "postgres"]), "remove");
    assert!(tools_line(&executor).ends_with("mcp__anvil__*, mcp__chrome-devtools__*"));
    let config = fs::read_to_string(repo.join(".harness/config.json")).unwrap();
    assert!(config.contains("mcp__chrome-devtools__*") && !config.contains("mcp__postgres__*"));

    let bad = anvil(&repo, &claude, &["orchestrator", "tools", "--add", "pg\nmodel: opus"]);
    assert!(!bad.status.success(), "a name with a newline must be refused");
    assert!(!fs::read_to_string(&executor).unwrap().contains("model: opus"));
}
