//! Env files, MCP servers and the secret scan, on temporary fixtures. No value or password may leak.

mod common;

use anvil_core::config::prereqs::{EnvFilePrereq, EnvVarPrereq, PathPrereq, PrereqSet};
use anvil_prereqs::{CheckOptions, Item, Kind, Report, ScriptedHost, Status, check, projects_with};
use common::{workspace, write};

fn run(root: &std::path::Path, host: &ScriptedHost, set: &PrereqSet) -> Report {
    check(host, root, &projects_with(&[]), set, &CheckOptions::default())
}

fn of_kind(report: &Report, kind: Kind) -> Vec<&Item> {
    report.items.iter().filter(|i| i.kind == kind).collect()
}

fn dump(report: &Report) -> String {
    serde_json::to_string(report).expect("json")
}

#[test]
fn env_file_with_bom_and_crlf_reports_only_missing_key_names() {
    let ws = workspace();
    let root = ws.path();
    write(
        root,
        ".env.example",
        "# config\nDATABASE_URL=\nJWT_SECRET=\nexport REDIS_URL=\nPORT=3000\n",
    );
    let mut real = vec![0xEF, 0xBB, 0xBF];
    real.extend_from_slice(b"# local\r\nDATABASE_URL=postgres://app:topsecret@localhost/db\r\nPORT=3001\r\n\r\nREDIS_URL=redis://localhost\r\n");
    write(root, ".env", &real);
    let set = PrereqSet {
        env_files: vec![EnvFilePrereq {
            file: ".env".to_owned(),
            example: ".env.example".to_owned(),
            subrepo: None,
        }],
        ..Default::default()
    };

    let report = run(root, &ScriptedHost::new(), &set);

    let item = &of_kind(&report, Kind::EnvFile)[0];
    assert_eq!(item.status, Status::Missing);
    assert_eq!(item.message.as_deref(), Some("missing keys: JWT_SECRET"));
    let text = dump(&report);
    assert!(!text.contains("topsecret") && !text.contains("localhost"), "values leaked: {text}");
}

#[test]
fn env_file_complete_and_missing_file() {
    let ws = workspace();
    let root = ws.path();
    write(root, ".env.example", "A=\nB=\n");
    write(root, ".env", "B=2\nA=1\n");
    write(root, "api/.env.example", "A=\n");
    let pair = |file: &str, example: &str| EnvFilePrereq {
        file: file.to_owned(),
        example: example.to_owned(),
        subrepo: None,
    };
    let set = PrereqSet {
        env_files: vec![pair(".env", ".env.example"), pair("api/.env", "api/.env.example")],
        ..Default::default()
    };

    let report = run(root, &ScriptedHost::new(), &set);

    assert_eq!(report.items[0].status, Status::Ok);
    assert_eq!(report.items[1].status, Status::Missing);
    assert_eq!(report.items[1].message.as_deref(), Some("file not found"));
    assert!(report.items[1].hint.as_deref().unwrap_or_default().contains("api/.env.example"));
}

#[test]
fn env_vars_come_from_the_environment_or_dot_env_and_paths_must_exist() {
    let ws = workspace();
    let root = ws.path();
    write(root, ".env", "API_KEY_SONAR=abc123\n");
    std::fs::create_dir_all(root.join(".venv")).expect("venv");
    let var = |name: &str, required: bool| EnvVarPrereq {
        name: name.to_owned(),
        source: ".env.example".to_owned(),
        subrepo: None,
        required,
        hint: String::new(),
    };
    let path = |p: &str, required: bool| PathPrereq {
        path: p.to_owned(),
        source: "pyproject.toml".to_owned(),
        subrepo: None,
        required,
        hint: "create it".to_owned(),
    };
    let set = PrereqSet {
        env_vars: vec![
            var("URL_SONAR", true),
            var("API_KEY_SONAR", true),
            var("SONAR_TOKEN", false),
        ],
        paths: vec![path(".venv", false), path("node_modules", false)],
        ..Default::default()
    };
    let host = ScriptedHost::new().env("URL_SONAR", "http://localhost:9000");

    let report = run(root, &host, &set);

    let statuses: Vec<Status> = report.items.iter().map(|i| i.status).collect();
    assert_eq!(statuses, [Status::Ok, Status::Ok, Status::Warning, Status::Ok, Status::Warning]);
    assert!(!dump(&report).contains("abc123"));
    assert!(report.ok);
}

fn mcp_set(names: &[&str]) -> PrereqSet {
    PrereqSet { mcp_servers: names.iter().map(|n| (*n).to_owned()).collect(), ..Default::default() }
}

#[test]
fn mcp_command_must_resolve_on_path() {
    let ws = workspace();
    let root = ws.path();
    write(
        root,
        ".mcp.json",
        r#"{"mcpServers":{"ok":{"command":"npx","args":["-y","chrome-devtools-mcp@latest"]},
            "gone":{"command":"uvx","args":["awslabs.aws-api-mcp-server@latest"]}}}"#,
    );
    let host = ScriptedHost::new().on_path("npx");

    let report = run(root, &host, &mcp_set(&["ok", "gone"]));

    let mcp = of_kind(&report, Kind::Mcp);
    let by_name = |n: &str| mcp.iter().find(|i| i.name == n).expect("item");
    assert_eq!(by_name("gone").status, Status::Missing);
    assert_eq!(by_name("gone").message.as_deref(), Some("command `uvx` not found on PATH"));
    assert_eq!(by_name("ok").status, Status::Ok);
    assert!(!report.ok);
}

#[test]
fn mcp_absolute_path_in_another_users_profile_is_flagged() {
    let ws = workspace();
    let root = ws.path();
    write(
        root,
        ".mcp.json",
        r#"{"mcpServers":{"chrome":{"command":"C:\\Users\\alice\\AppData\\Roaming\\npm\\npx.cmd","args":["-y","chrome-devtools-mcp"]}}}"#,
    );
    let host = ScriptedHost::new().env("USERNAME", "bob");

    let report = run(root, &host, &mcp_set(&["chrome"]));

    let item = &of_kind(&report, Kind::Mcp)[0];
    let message = item.message.clone().unwrap_or_default();
    assert!(message.contains("another user's profile (`alice`)"), "message: {message}");
    assert_ne!(item.status, Status::Ok);
    assert!(item.hint.as_deref().unwrap_or_default().contains("npx"));
}

#[test]
fn mcp_path_in_the_current_users_profile_is_not_foreign() {
    let ws = workspace();
    let root = ws.path();
    write(root, ".mcp.json", r#"{"mcpServers":{"s":{"command":"/home/bob/bin/mcp-server"}}}"#);
    let host = ScriptedHost::new().env("USER", "bob");

    let report = run(root, &host, &mcp_set(&["s"]));

    let message = of_kind(&report, Kind::Mcp)[0].message.clone().unwrap_or_default();
    assert!(!message.contains("another user"), "message: {message}");
    assert!(message.contains("does not exist"), "message: {message}");
}

#[test]
fn mcp_archived_packages_are_flagged_with_a_replacement() {
    let ws = workspace();
    let root = ws.path();
    write(
        root,
        ".mcp.json",
        r#"{"mcpServers":{
            "db":{"command":"npx","args":["-y","@modelcontextprotocol/server-postgres","$DATABASE_URL"]},
            "gh":{"command":"npx","args":["-y","@modelcontextprotocol/server-github"]},
            "fine":{"command":"npx","args":["-y","@playwright/mcp@latest"]}}}"#,
    );
    let host = ScriptedHost::new().on_path("npx");

    let report = run(root, &host, &mcp_set(&["db", "gh", "fine"]));

    let by_name =
        |n: &str| of_kind(&report, Kind::Mcp).into_iter().find(|i| i.name == n).expect("item");
    let db = by_name("db");
    assert_eq!(db.status, Status::Warning);
    assert!(
        db.message.as_deref().unwrap_or_default().contains("@modelcontextprotocol/server-postgres")
    );
    assert!(db.message.as_deref().unwrap_or_default().contains("archived"));
    assert!(db.hint.as_deref().unwrap_or_default().contains("postgres-mcp"));
    assert!(by_name("gh").hint.as_deref().unwrap_or_default().contains("github-mcp-server"));
    assert_eq!(by_name("fine").status, Status::Ok);
    assert!(report.ok, "warnings do not fail the check");
}

#[test]
fn mcp_inline_credentials_are_flagged_without_printing_the_password() {
    let ws = workspace();
    let root = ws.path();
    write(
        root,
        ".mcp.json",
        r#"{"mcpServers":{
            "db":{"command":"npx","args":["-y","pg-mcp","postgres://app:Sup3rS3cret@db.internal:5432/orders"]},
            "cache":{"command":"npx","args":["redis-mcp"],"env":{"REDIS_URL":"redis://default:hunter2@cache.internal"}},
            "safe":{"command":"npx","args":["pg-mcp","postgres://app:${DB_PASSWORD}@db.internal/orders"]}}}"#,
    );
    let host = ScriptedHost::new().on_path("npx");

    let report = run(root, &host, &mcp_set(&["db", "cache", "safe"]));

    let mcp = of_kind(&report, Kind::Mcp);
    let status = |n: &str| mcp.iter().find(|i| i.name == n).expect("item").status;
    assert_eq!(status("db"), Status::Warning);
    assert_eq!(status("cache"), Status::Warning);
    assert_eq!(status("safe"), Status::Ok, "a variable reference is not a credential");
    let db = mcp.iter().find(|i| i.name == "db").expect("db");
    assert!(db.message.as_deref().unwrap_or_default().contains("postgres://db.internal"));
    let text = dump(&report);
    for secret in ["Sup3rS3cret", "hunter2", "app:"] {
        assert!(!text.contains(secret), "`{secret}` leaked into: {text}");
    }
}

#[test]
fn ignored_servers_are_not_checked() {
    let ws = workspace();
    let root = ws.path();
    write(root, ".mcp.json", r#"{"mcpServers":{"gone":{"command":"missing-bin"}}}"#);
    let report = run(root, &ScriptedHost::new(), &mcp_set(&[]));
    assert!(of_kind(&report, Kind::Mcp).is_empty());
}

#[test]
fn secret_scan_finds_connection_strings_in_scripts_and_agent_configs() {
    let ws = workspace();
    let root = ws.path();
    write(
        root,
        "scripts/db-reset.sh",
        "#!/bin/sh\necho start\npsql postgres://u:p@h/db -c 'select 1'\npsql \"postgres://u:${PGPASS}@h/db\"\n",
    );
    write(root, "harness/nested/seed.py", "URL = 'mysql://root:rootpw@127.0.0.1:3306/app'\n");
    write(
        root,
        ".claude/settings.local.json",
        r#"{"env":{"DB":"mongodb://admin:adminpw@mongo.local/x"}}"#,
    );
    write(root, "scripts/clean.sh", "echo no credentials here, only http://example.com/path\n");
    write(root, "scripts/blob.bin", b"postgres://bin:binpw@h/db\0\0");
    write(
        root,
        "scripts/huge.sh",
        format!("{}postgres://big:bigpw@h/db\n", "x".repeat(600 * 1024)),
    );

    let report = run(root, &ScriptedHost::new(), &PrereqSet::default());

    let secrets = of_kind(&report, Kind::Secret);
    let names: Vec<&str> = secrets.iter().map(|i| i.name.as_str()).collect();
    assert!(names.contains(&"scripts/db-reset.sh:3"), "findings: {names:?}");
    assert!(names.contains(&"harness/nested/seed.py:1"), "findings: {names:?}");
    assert!(names.contains(&".claude/settings.local.json:1"), "findings: {names:?}");
    assert_eq!(
        names.len(),
        3,
        "variable references, binaries and files over 512 KB are skipped: {names:?}"
    );
    assert!(secrets.iter().all(|i| i.status == Status::Warning));
    let first = secrets.iter().find(|i| i.name == "scripts/db-reset.sh:3").expect("finding");
    assert!(first.message.as_deref().unwrap_or_default().contains("postgres://h"));
    assert!(report.ok, "the secret scan only warns");
    let text = dump(&report);
    for secret in ["u:p", "rootpw", "adminpw", "binpw", "bigpw"] {
        assert!(!text.contains(secret), "`{secret}` leaked into: {text}");
    }
}
