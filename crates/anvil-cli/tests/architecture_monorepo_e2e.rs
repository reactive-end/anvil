//! `anvil init --architecture` in a monorepo: the preset only goes where it fits, and the agent files
//! state each style once.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

fn anvil(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_anvil"))
        .arg("--root")
        .arg(root)
        .args(args)
        .env("NO_COLOR", "1")
        .env("ANVIL_NO_AUTO_DAEMON", "1")
        .env("ANVIL_LANG", "en")
        .output()
        .expect("spawn anvil")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn describe(out: &Output) -> String {
    format!("stdout:\n{}\nstderr:\n{}", stdout(out), String::from_utf8_lossy(&out.stderr))
}

fn ok(root: &Path, args: &[&str]) -> Output {
    let out = anvil(root, args);
    assert!(out.status.success(), "anvil {args:?} failed\n{}", describe(&out));
    out
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, text).expect("write");
}

/// A Nest-like hexagonal `backend` next to a Vite `frontend`.
fn monorepo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write(
        root,
        "backend/package.json",
        r#"{"name":"backend","dependencies":{"@nestjs/core":"^10"}}"#,
    );
    write(root, "backend/src/orders/domain/order.ts", "export class Order {}\n");
    write(root, "backend/src/orders/application/create.ts", "export class Create {}\n");
    write(root, "backend/src/orders/infrastructure/repo.ts", "export class Repo {}\n");
    write(root, "frontend/package.json", r#"{"name":"frontend","devDependencies":{"vite":"^5"}}"#);
    write(root, "frontend/vite.config.ts", "export default {};\n");
    write(root, "frontend/src/main.ts", "console.log('hi');\n");
    dir
}

fn init_monorepo(root: &Path, extra: &[&str]) -> Output {
    let mut args = vec!["init", "--adapter", "claude", "--layout", "monorepo", "--sonar", "no"];
    args.extend_from_slice(extra);
    ok(root, &args)
}

fn config(root: &Path) -> Value {
    let raw = fs::read_to_string(root.join(".harness/config.json")).expect("config");
    serde_json::from_str(&raw).expect("json")
}

fn architecture_lines(root: &Path) -> Vec<String> {
    let claude = fs::read_to_string(root.join("CLAUDE.md")).expect("CLAUDE.md");
    claude.lines().filter(|l| l.contains("Architecture:")).map(str::to_owned).collect()
}

#[test]
fn the_preset_goes_only_to_the_backend_and_the_frontend_is_reported() {
    let dir = monorepo();
    let root = dir.path();

    let init = init_monorepo(root, &["--architecture", "hexagonal"]);

    let cfg = config(root);
    let subrepos = cfg["architecture"]["subrepos"].as_object().expect("subrepos");
    assert_eq!(subrepos.keys().collect::<Vec<_>>(), ["backend"], "{cfg}");
    assert_eq!(subrepos["backend"]["style"], "hexagonal");
    assert_ne!(cfg["architecture"]["style"], "hexagonal", "no root style next to subrepo ones");
    let text = stdout(&init);
    assert!(text.contains("frontend: vite client app"), "{text}");
    assert!(text.contains("anvil architecture set <style> --subrepo frontend"), "{text}");
    let lines = architecture_lines(root);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("hexagonal (ports & adapters) in backend:"), "{}", lines[0]);
}

#[test]
fn a_root_with_its_own_src_gets_no_style_when_a_subrepo_has_one() {
    let dir = monorepo();
    let root = dir.path();
    write(root, "src/index.ts", "export {};\n");

    init_monorepo(root, &["--architecture", "hexagonal"]);

    let cfg = config(root);
    assert_ne!(cfg["architecture"]["style"], "hexagonal", "{cfg}");
    assert_eq!(cfg["architecture"]["subrepos"]["backend"]["style"], "hexagonal");
    assert_eq!(architecture_lines(root).len(), 1);
}

#[test]
fn an_explicit_choice_already_in_the_config_is_kept() {
    let dir = monorepo();
    let root = dir.path();
    init_monorepo(root, &[]);
    ok(root, &["architecture", "set", "layered", "--subrepo", "backend"]);
    ok(root, &["architecture", "set", "feature-sliced", "--subrepo", "frontend"]);

    init_monorepo(root, &["--architecture", "hexagonal"]);

    let cfg = config(root);
    assert_eq!(cfg["architecture"]["subrepos"]["backend"]["style"], "layered", "{cfg}");
    assert_eq!(cfg["architecture"]["subrepos"]["frontend"]["style"], "feature-sliced", "{cfg}");
}

#[test]
fn a_single_client_app_repository_still_takes_the_preset() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write(root, "package.json", r#"{"name":"web","devDependencies":{"vite":"^5"}}"#);
    write(root, "src/main.ts", "console.log('hi');\n");

    ok(root, &["init", "--adapter", "claude", "--architecture", "hexagonal"]);

    assert_eq!(config(root)["architecture"]["style"], "hexagonal");
    assert_eq!(architecture_lines(root).len(), 1);
}

#[test]
fn a_legacy_root_plus_subrepos_config_still_loads_checks_and_renders_one_line() {
    let dir = monorepo();
    let root = dir.path();
    init_monorepo(root, &[]);
    ok(root, &["architecture", "set", "hexagonal", "--source-root", "backend/src"]);
    ok(root, &["architecture", "set", "hexagonal", "--subrepo", "backend"]);
    ok(root, &["architecture", "set", "hexagonal", "--subrepo", "frontend"]);

    ok(root, &["sync"]);
    ok(root, &["architecture", "check", "--json"]);

    let lines = architecture_lines(root);
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("in this repository, backend, frontend:"), "{}", lines[0]);
}
