//! `anvil docs` end to end in temporary git repositories: the CLI, the reactive hooks, and the
//! guard and role rules around `anvil docs verify`.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

const SERVICE: &str = "export class OrdersService {\n  create() {}\n}\n";
const SESSION: &str = "sess-docs";

fn run(root: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_anvil"))
        .arg("--root")
        .arg(root)
        .args(args)
        .current_dir(root)
        .env("ANVIL_LANG", "en")
        .env("NO_COLOR", "1")
        .env("ANVIL_NO_AUTO_DAEMON", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn anvil");
    if let (Some(data), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(data.as_bytes()).expect("stdin");
    }
    child.wait_with_output().expect("wait")
}

fn ok(root: &Path, args: &[&str]) -> String {
    let out = run(root, args, None);
    assert!(
        out.status.success(),
        "anvil {args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git").arg("-C").arg(root).args(args).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, text).expect("write");
}

fn repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "t@example.com"]);
    git(root, &["config", "user.name", "Tester"]);
    write(root, ".gitignore", "target/\nanvil-home/\n");
    write(root, "package.json", r#"{"name":"shop","version":"1.0.0"}"#);
    write(root, "src/orders/orders.service.ts", SERVICE);
    write(root, "src/billing/invoice.ts", "export function bill() {}\n");
    ok(root, &["init"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "initial"]);
    dir
}

fn json_of(root: &Path, args: &[&str]) -> Value {
    serde_json::from_str(&ok(root, args)).expect("json output")
}

fn project_of(root: &Path) -> String {
    let rows = json_of(root, &["docs", "list", "--json"]);
    rows[0]["project"].as_str().expect("project").to_owned()
}

fn row<'a>(rows: &'a Value, id_suffix: &str) -> &'a Value {
    rows.as_array()
        .and_then(|all| {
            all.iter().find(|r| r["id"].as_str().is_some_and(|i| i.ends_with(id_suffix)))
        })
        .unwrap_or_else(|| panic!("no concept ending in {id_suffix}: {rows}"))
}

fn doc_path(root: &Path, project: &str, module: &str) -> std::path::PathBuf {
    root.join(".harness/docs").join(project).join(module).join("overview.md")
}

/// A repository with the bundle initialised and the `orders` concept created; returns its project name.
fn documented() -> (TempDir, String) {
    let dir = repo();
    let root = dir.path();
    let first = ok(root, &["docs", "init"]);
    assert!(first.contains("initialised"), "{first}");
    assert!(ok(root, &["docs", "init"]).contains("already initialised"));
    ok(
        root,
        &[
            "docs",
            "new",
            "orders",
            "--paths",
            "src/orders/**",
            "--title",
            "Orders",
            "--description",
            "Order placement and lifecycle",
        ],
    );
    let project = project_of(root);
    (dir, project)
}

#[test]
fn init_and_new_write_the_bundle_and_a_concept_with_all_sections() {
    let (dir, project) = documented();
    let root = dir.path();
    let rows = json_of(root, &["docs", "list", "--json"]);
    let orders = row(&rows, "/orders");
    assert_eq!(orders["status"], "draft");
    assert_eq!(orders["trust"], "unverified");
    assert_eq!(orders["stale"], false);
    let index = fs::read_to_string(root.join(".harness/docs").join(&project).join("index.md"))
        .expect("index");
    assert!(index.contains("okf_version: \"0.3\""));
    assert!(index.contains("[orders](orders/overview.md)"));
    let log =
        fs::read_to_string(root.join(".harness/docs").join(&project).join("log.md")).expect("log");
    assert!(log.contains("Created [orders](orders/overview.md)."), "{log}");
    let doc = fs::read_to_string(doc_path(root, &project, "orders")).expect("doc");
    for needle in ["type: Code Module", "x-anvil-synced-commit:", "generated:", "x-anvil-paths:"] {
        assert!(
            doc.contains(needle),
            "missing {needle}:
{doc}"
        );
    }
    for heading in
        ["Purpose", "Responsibilities", "Public interface", "Invariants", "Dependencies", "Related"]
    {
        assert!(doc.contains(&format!("## {heading}")), "missing {heading}");
    }
    assert!(!doc.contains("TODO") && !doc.contains("FIXME"));
    ok(root, &["docs", "check", "--strict"]);
}

#[test]
fn code_changes_make_a_concept_stale_until_it_is_refreshed() {
    let (dir, project) = documented();
    let root = dir.path();
    write(
        root,
        "src/orders/orders.service.ts",
        "export class OrdersService {
  create() { return 1 }
}
",
    );
    assert_eq!(row(&json_of(root, &["docs", "list", "--json"]), "/orders")["stale"], true);
    assert!(ok(root, &["docs", "check"]).contains("stale"));
    assert_eq!(run(root, &["docs", "check", "--strict"], None).status.code(), Some(1));

    let refreshed = ok(root, &["docs", "refresh", "--stale"]);
    assert!(refreshed.contains(&format!("refreshed {project}/orders")), "{refreshed}");
    ok(root, &["docs", "check", "--strict"]);
    assert!(ok(root, &["docs", "refresh", "--stale"]).contains("nothing to refresh"));
}

#[test]
fn broken_links_fail_the_check() {
    let (dir, project) = documented();
    let root = dir.path();
    let path = doc_path(root, &project, "orders");
    let mut text = fs::read_to_string(&path).expect("doc");
    text.push_str(
        "
See [missing](nowhere.md).
",
    );
    fs::write(&path, &text).expect("write");
    let failed = run(root, &["docs", "check"], None);
    assert_eq!(failed.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&failed.stdout).contains("broken link `nowhere.md`"));
    fs::write(&path, text.replace("[missing](nowhere.md)", "missing")).expect("fix");
    ok(root, &["docs", "check"]);
}

#[test]
fn verify_raises_the_trust_tier_and_search_finds_the_concept() {
    let (dir, project) = documented();
    let root = dir.path();
    ok(root, &["docs", "verify", &format!("{project}/orders"), "--by", "Ada"]);
    let verified = json_of(root, &["docs", "list", "--json"]);
    assert_eq!(row(&verified, "/orders")["trust"], "human-reviewed");

    let hits = json_of(root, &["docs", "search", "placement lifecycle", "--json"]);
    assert_eq!(hits[0]["id"], format!("{project}/orders"));
    assert!(hits[0]["snippet"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(ok(root, &["docs", "search", "zebra"]).contains("no match"));
}

#[test]
fn scaffold_documents_nest_modules_with_layers_and_outline() {
    let dir = repo();
    let root = dir.path();
    write(root, "src/orders/domain/order.ts", "export class Order {}\n");
    write(root, "src/orders/application/create-order.ts", "export function createOrder() {}\n");
    write(root, "src/orders/infrastructure/repo.ts", "export class OrderRepo {}\n");
    write(root, "src/orders/controller/orders.controller.ts", "export class OrdersController {}\n");
    ok(root, &["graph", "build"]);
    ok(root, &["docs", "init"]);
    let created = ok(root, &["docs", "scaffold"]);
    assert!(created.contains("2 concept(s) created"), "{created}");

    let rows = json_of(root, &["docs", "list", "--json"]);
    let project = project_of(root);
    assert_eq!(rows.as_array().map(Vec::len), Some(2), "{rows}");
    row(&rows, "/billing");
    row(&rows, "/orders");
    let doc = fs::read_to_string(doc_path(root, &project, "orders")).expect("doc");
    for layer in [
        "`application/` layer",
        "`controller/` layer",
        "`domain/` layer",
        "`infrastructure/` layer",
    ] {
        assert!(doc.contains(layer), "missing {layer}:\n{doc}");
    }
    assert!(
        doc.contains("<!-- anvil:outline:start -->") && doc.contains("`OrdersService`"),
        "{doc}"
    );
    ok(root, &["docs", "check"]);

    let again = ok(root, &["docs", "scaffold"]);
    assert!(again.contains("0 concept(s) created"), "{again}");
    fs::remove_file(root.join(".harness/.state/graph.bin")).expect("remove graph");
    let out = run(root, &["docs", "scaffold"], None);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("anvil graph build"));
}

fn hook(root: &Path, event: &str, payload: &Value) -> Value {
    let out = run(root, &["hook", event, "--adapter", "claude"], Some(&payload.to_string()));
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).expect("hook json")
}

fn edit_payload(root: &Path, rel: &str) -> Value {
    json!({
        "hook_event_name": "PostToolUse",
        "session_id": SESSION,
        "cwd": root.to_string_lossy(),
        "tool_name": "Edit",
        "tool_input": { "file_path": rel },
    })
}

fn context(out: &Value) -> String {
    out["hookSpecificOutput"]["additionalContext"].as_str().unwrap_or_default().to_owned()
}

#[test]
fn editing_a_documented_file_reminds_once_per_turn() {
    let dir = repo();
    let root = dir.path();
    ok(root, &["docs", "new", "orders", "--paths", "src/orders/**"]);
    let project = project_of(root);
    let reminder = format!(
        ".harness/docs/{project}/orders/overview.md` documents this file: update it if behaviour changed, then run `anvil docs refresh {project}/orders`."
    );

    write(
        root,
        "src/orders/orders.service.ts",
        "export class OrdersService {\n  create() { return 2 }\n}\n",
    );
    let payload = edit_payload(root, "src/orders/orders.service.ts");
    let first = context(&hook(root, "post-tool-use", &payload));
    assert!(first.contains(&reminder), "{first}");
    let second = context(&hook(root, "post-tool-use", &payload));
    assert!(!second.contains("documents this file"), "{second}");

    let unrelated = edit_payload(root, "src/billing/invoice.ts");
    assert!(!context(&hook(root, "post-tool-use", &unrelated)).contains("documents this file"));

    // A new user prompt starts a new turn, so the reminder comes back.
    let prompt = json!({ "hook_event_name": "UserPromptSubmit", "session_id": SESSION, "cwd": root.to_string_lossy(), "prompt": "next" });
    hook(root, "user-prompt-submit", &prompt);
    assert!(context(&hook(root, "post-tool-use", &payload)).contains(&reminder));
}

#[test]
fn editing_a_concept_reports_validation_errors() {
    let dir = repo();
    let root = dir.path();
    ok(root, &["docs", "new", "orders", "--paths", "src/orders/**"]);
    let project = project_of(root);
    let rel = format!(".harness/docs/{project}/orders/overview.md");
    let good = context(&hook(root, "post-tool-use", &edit_payload(root, &rel)));
    assert!(!good.contains("not valid OKF"), "{good}");

    let mut text = fs::read_to_string(root.join(&rel)).expect("doc");
    text.push_str("\n[gone](missing.md)\n");
    fs::write(root.join(&rel), text).expect("write");
    let bad = context(&hook(root, "post-tool-use", &edit_payload(root, &rel)));
    assert!(bad.contains("not valid OKF") && bad.contains("broken link `missing.md`"), "{bad}");
}

#[test]
fn hooks_stay_silent_without_bundles() {
    let dir = repo();
    let root = dir.path();
    write(root, ".harness/docs/PLAN.md", "# Plan\n");
    let out =
        context(&hook(root, "post-tool-use", &edit_payload(root, "src/orders/orders.service.ts")));
    assert!(!out.contains("documents this file"), "{out}");
}

fn pre_tool(root: &Path, command: &str, agent: Option<&str>) -> String {
    let mut payload = json!({
        "hook_event_name": "PreToolUse",
        "session_id": SESSION,
        "cwd": root.to_string_lossy(),
        "tool_name": "Bash",
        "tool_use_id": "toolu_docs",
        "tool_input": { "command": command },
    });
    if let Some(id) = agent {
        payload["agent_id"] = json!(id);
        payload["agent_type"] = json!("anvil-executor");
    }
    let out = hook(root, "pre-tool-use", &payload);
    out["hookSpecificOutput"]["permissionDecision"].as_str().unwrap_or("none").to_owned()
}

#[test]
fn verify_always_asks_the_user() {
    let dir = repo();
    let root = dir.path();
    for command in [
        "anvil docs verify shop/orders",
        "anvil --root . docs verify shop/orders --by Ada",
        "bash -c \"anvil docs verify shop/orders\"",
    ] {
        assert_eq!(pre_tool(root, command, None), "ask", "{command}");
    }
    for command in ["anvil docs list", "anvil docs check", "anvil docs refresh shop/orders"] {
        assert_ne!(pre_tool(root, command, None), "ask", "{command}");
    }
}

#[test]
fn orchestrator_mode_refuses_verify_for_every_role() {
    let dir = repo();
    let root = dir.path();
    ok(root, &["orchestrator", "on"]);
    assert_eq!(pre_tool(root, "anvil docs verify shop/orders", None), "deny");
    assert_eq!(pre_tool(root, "anvil docs verify shop/orders --by Ada", Some("agent-1")), "deny");
    for command in ["anvil docs list", "anvil docs check --strict", "anvil docs search orders"] {
        assert_ne!(pre_tool(root, command, None), "deny", "orchestrator: {command}");
        assert_ne!(pre_tool(root, command, Some("agent-1")), "deny", "executor: {command}");
    }
    assert_eq!(pre_tool(root, "anvil docs refresh shop/orders", None), "deny");
    assert_ne!(pre_tool(root, "anvil docs refresh shop/orders", Some("agent-1")), "deny");
}

#[test]
fn anvil_check_reports_invalid_docs_as_errors_and_stale_concepts_as_warnings() {
    let dir = repo();
    let root = dir.path();
    ok(root, &["docs", "new", "orders", "--paths", "src/orders/**"]);
    let project = project_of(root);
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "docs"]);

    write(
        root,
        "src/orders/orders.service.ts",
        "export class OrdersService {\n  create() { return 3 }\n}\n",
    );
    let stale = run(root, &["check", "--tier", "0", "--scope", "src"], None);
    let text = String::from_utf8_lossy(&stale.stdout).into_owned();
    assert!(stale.status.success(), "{text}");
    assert!(
        text.contains("docs/stale")
            && text.contains(&format!("anvil docs refresh {project}/orders")),
        "{text}"
    );

    let rel = format!(".harness/docs/{project}/orders/overview.md");
    let mut doc = fs::read_to_string(root.join(&rel)).expect("doc");
    doc.push_str("\n[gone](missing.md)\n");
    fs::write(root.join(&rel), doc).expect("write");
    let invalid = run(root, &["check", "--tier", "0", "--scope", ".harness/docs"], None);
    let text = String::from_utf8_lossy(&invalid.stdout).into_owned();
    assert_eq!(invalid.status.code(), Some(1), "{text}");
    assert!(text.contains("docs/okf") && text.contains("broken link `missing.md`"), "{text}");
}

#[test]
fn stop_advises_about_concepts_that_were_not_refreshed() {
    let dir = repo();
    let root = dir.path();
    ok(root, &["docs", "new", "orders", "--paths", "src/orders/**"]);
    let project = project_of(root);
    write(
        root,
        "src/orders/orders.service.ts",
        "export class OrdersService {\n  create() { return 4 }\n}\n",
    );
    hook(root, "post-tool-use", &edit_payload(root, "src/orders/orders.service.ts"));
    let stop =
        json!({ "hook_event_name": "Stop", "session_id": SESSION, "cwd": root.to_string_lossy() });
    let out = hook(root, "stop", &stop).to_string();
    assert!(
        out.contains(&format!("{project}/orders")) && out.contains("was not refreshed"),
        "{out}"
    );
}

#[test]
fn a_monorepo_gets_one_bundle_per_subrepo() {
    let dir = repo();
    let root = dir.path();
    write(root, "apps/api/src/orders/orders.service.ts", SERVICE);
    write(root, "apps/web/src/home/home.ts", "export function home() {}\n");
    let config_path = root.join(".harness/config.json");
    let mut config: Value =
        serde_json::from_str(&fs::read_to_string(&config_path).expect("config")).expect("json");
    config["subrepos"] = json!([
        { "name": "api", "path": "apps/api" },
        { "name": "web", "path": "apps/web" }
    ]);
    fs::write(&config_path, config.to_string()).expect("write config");

    ok(root, &["docs", "init"]);
    assert!(root.join(".harness/docs/api/index.md").is_file());
    assert!(root.join(".harness/docs/web/index.md").is_file());
    let ambiguous =
        run(root, &["docs", "new", "orders", "--paths", "apps/api/src/orders/**"], None);
    assert!(String::from_utf8_lossy(&ambiguous.stderr).contains("--project"));
    ok(root, &["graph", "build"]);
    ok(root, &["docs", "scaffold"]);
    let api = fs::read_to_string(doc_path(root, "api", "orders")).expect("api orders");
    assert!(api.contains("apps/api/src/orders/**"), "{api}");
    assert!(doc_path(root, "web", "home").is_file());
    assert!(!doc_path(root, "api", "home").exists());
    ok(root, &["docs", "check"]);
}
