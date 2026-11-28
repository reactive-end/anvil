//! `anvil architecture`, `anvil init --architecture`, and `anvil sync` end to end on a tiny hexagonal tree.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

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

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, text).expect("write");
}

/// A NestJS-like backend with two hexagonal modules; every import already respects the layers.
fn hexagonal_project() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write(
        root,
        "package.json",
        r#"{"name":"orders-api","dependencies":{"@nestjs/common":"^10.0.0"}}"#,
    );
    write(root, "tsconfig.json", r#"{"compilerOptions":{"strict":true}}"#);
    write(
        root,
        "src/orders/domain/order.ts",
        "export class Order {\n  constructor(readonly id: string) {}\n}\n",
    );
    write(
        root,
        "src/orders/domain/order.repository.ts",
        "import { Order } from './order';\nexport interface IOrderRepository {\n  save(order: Order): Promise<void>;\n}\n",
    );
    write(
        root,
        "src/orders/application/create-order.use-case.ts",
        "import { Inject, Injectable } from '@nestjs/common';\nimport { Order } from '../domain/order';\nimport type { IOrderRepository } from '../domain/order.repository';\nimport { Customer } from '../../customers/domain/customer';\n\n@Injectable()\nexport class CreateOrder {\n  constructor(@Inject('IOrderRepository') private readonly repo: IOrderRepository) {}\n  run(c: Customer) { return this.repo.save(new Order(c.id)); }\n}\n",
    );
    write(
        root,
        "src/orders/infrastructure/pg-order.repository.ts",
        "import { Order } from '../domain/order';\nimport type { IOrderRepository } from '../domain/order.repository';\nimport { Repository } from 'typeorm';\n\nexport class PgOrderRepository implements IOrderRepository {\n  constructor(private readonly db: Repository<Order>) {}\n  async save(order: Order) { await this.db.save(order); }\n}\n",
    );
    write(
        root,
        "src/orders/orders.module.ts",
        "import { PgOrderRepository } from './infrastructure/pg-order.repository';\nimport { CreateOrder } from './application/create-order.use-case';\nexport const OrdersModule = { providers: [PgOrderRepository, CreateOrder] };\n",
    );
    write(
        root,
        "src/customers/domain/customer.ts",
        "export class Customer {\n  constructor(readonly id: string) {}\n}\n",
    );
    write(
        root,
        "src/customers/infrastructure/orm.ts",
        "import { Customer } from '../domain/customer';\nexport const table = (c: Customer) => c.id;\n",
    );
    write(
        root,
        "src/app.module.ts",
        "import { OrdersModule } from './orders/orders.module';\nexport const AppModule = { imports: [OrdersModule] };\n",
    );
    dir
}

fn check_json(root: &Path) -> (bool, Value) {
    let out = anvil(root, &["architecture", "check", "--json"]);
    let report: Value =
        serde_json::from_str(&stdout(&out)).unwrap_or_else(|e| panic!("{e}: {}", describe(&out)));
    (out.status.success(), report)
}

type Row = (String, u64, String, String);

fn row(file: &str, line: u64, target: &str, rule: &str) -> Row {
    (file.to_owned(), line, target.to_owned(), rule.to_owned())
}

fn violations(report: &Value) -> Vec<Row> {
    let mut rows: Vec<_> = report["violations"]
        .as_array()
        .expect("violations array")
        .iter()
        .map(|v| {
            (
                v["file"].as_str().expect("file").to_owned(),
                v["line"].as_u64().expect("line"),
                v["target"].as_str().expect("target").to_owned(),
                v["rule"].as_str().expect("rule").to_owned(),
            )
        })
        .collect();
    rows.sort();
    rows
}

/// A compliant tree with the hexagonal style recorded and the graph built.
fn configured_project() -> TempDir {
    let dir = hexagonal_project();
    let set = anvil(dir.path(), &["architecture", "set", "hexagonal"]);
    assert!(set.status.success(), "{}", describe(&set));
    let build = anvil(dir.path(), &["graph", "build"]);
    assert!(build.status.success(), "{}", describe(&build));
    dir
}

#[test]
fn detect_only_suggests_and_set_records_the_style() {
    let dir = hexagonal_project();
    let root = dir.path();

    let detect = anvil(root, &["architecture", "detect"]);
    assert!(detect.status.success(), "{}", describe(&detect));
    let text = stdout(&detect);
    assert!(text.contains("hexagonal (medium confidence)"), "{text}");
    assert!(text.contains("anvil architecture set hexagonal"), "{text}");
    assert!(!root.join(".harness/config.json").exists(), "detect must not write anything");

    let set = anvil(root, &["architecture", "set", "hexagonal"]);
    assert!(set.status.success(), "{}", describe(&set));
    let raw = fs::read_to_string(root.join(".harness/config.json")).expect("config");
    let config: Value = serde_json::from_str(&raw).expect("json");
    assert_eq!(config["architecture"]["style"], "hexagonal");
}

#[test]
fn a_compliant_tree_passes_the_check() {
    let dir = configured_project();

    let out = anvil(dir.path(), &["architecture", "check"]);
    assert!(out.status.success(), "{}", describe(&out));
    assert!(
        stdout(&out).contains("architecture ok: hexagonal (this repository)"),
        "{}",
        stdout(&out)
    );

    let (ok, report) = check_json(dir.path());
    assert!(ok, "{report}");
    assert_eq!(report["ok"], true);
    assert_eq!(violations(&report), []);
}

#[test]
fn violations_fail_with_file_line_target_and_the_reason() {
    let dir = configured_project();
    let root = dir.path();
    write(
        root,
        "src/orders/domain/order.ts",
        "import { PgOrderRepository } from '../infrastructure/pg-order.repository';\nimport { Injectable } from '@nestjs/common';\n\n@Injectable()\nexport class Order {\n  constructor(readonly id: string, readonly repo?: PgOrderRepository) {}\n}\n",
    );
    write(
        root,
        "src/orders/application/peek.ts",
        "import { table } from '../../customers/infrastructure/orm';\nexport const peek = table;\n",
    );
    assert!(anvil(root, &["graph", "build"]).status.success());

    let out = anvil(root, &["architecture", "check"]);
    assert_eq!(out.status.code(), Some(1), "{}", describe(&out));
    let text = stdout(&out);
    let repo_import =
        "src/orders/domain/order.ts:1 -> src/orders/infrastructure/pg-order.repository.ts";
    assert!(text.contains(repo_import), "{text}");
    assert!(text.contains("src/orders/domain/order.ts:2 -> @nestjs/common"), "{text}");

    let (ok, report) = check_json(root);
    assert!(!ok);
    let orm = "src/customers/infrastructure/orm.ts";
    let repo = "src/orders/infrastructure/pg-order.repository.ts";
    let peek = "src/orders/application/peek.ts";
    let order = "src/orders/domain/order.ts";
    assert_eq!(
        violations(&report),
        [
            row(peek, 1, orm, "arch:hexagonal:application-no-infrastructure"),
            row(peek, 1, orm, "arch:hexagonal:module-boundaries"),
            row(order, 1, repo, "arch:hexagonal:domain-no-infrastructure"),
            row(order, 2, "@nestjs/common", "arch:hexagonal:domain-no-framework"),
        ],
        "{report}"
    );
    let reasons = report["violations"].as_array().expect("violations");
    assert!(reasons.iter().all(|v| !v["reason"].as_str().unwrap_or("").is_empty()));
}

#[test]
fn sync_writes_the_architecture_line_into_claude_md() {
    let dir = configured_project();
    let root = dir.path();

    let sync = anvil(root, &["sync"]);
    assert!(sync.status.success(), "{}", describe(&sync));

    let claude = fs::read_to_string(root.join("CLAUDE.md")).expect("CLAUDE.md");
    let line = claude
        .lines()
        .find(|l| l.contains("Architecture: hexagonal"))
        .unwrap_or_else(|| panic!("no architecture line in:\n{claude}"));
    assert!(line.contains("(ports & adapters) in this repository"), "{line}");
    assert!(line.contains("wire them in *.module.ts"), "{line}");
}

#[test]
fn show_lists_rules_and_set_validates_the_style_and_subrepo() {
    let dir = hexagonal_project();
    let root = dir.path();
    assert!(
        anvil(root, &["architecture", "set", "hexagonal", "--source-root", "src"]).status.success()
    );

    let show = anvil(root, &["architecture", "show", "--json"]);
    let report: Value = serde_json::from_str(&stdout(&show)).expect("show json");
    let target = &report["targets"][0];
    assert_eq!(target["style"], "hexagonal");
    assert_eq!(target["source_dir"], "src");
    let names: Vec<&str> = target["rules"]
        .as_array()
        .expect("rules")
        .iter()
        .map(|r| r["name"].as_str().expect("name"))
        .collect();
    assert!(names.contains(&"arch:hexagonal:domain-no-framework"), "{names:?}");
    assert!(target["guidance"].as_str().expect("guidance").contains("use cases depend on ports"));

    let text = stdout(&anvil(root, &["architecture", "show"]));
    assert!(text.contains("arch:hexagonal:module-boundaries"), "{text}");

    let bad_style = anvil(root, &["architecture", "set", "spaghetti"]);
    assert!(!bad_style.status.success());
    assert!(String::from_utf8_lossy(&bad_style.stderr).contains("unknown architecture style"));
    let bad_sub = anvil(root, &["architecture", "set", "clean", "--subrepo", "nope"]);
    assert!(!bad_sub.status.success());
    assert!(String::from_utf8_lossy(&bad_sub.stderr).contains("unknown subrepo"));

    assert!(anvil(root, &["architecture", "set", "none"]).status.success());
    let after = fs::read_to_string(root.join(".harness/config.json")).expect("config");
    assert!(!after.contains("architecture"), "none removes the section: {after}");
}

#[test]
fn init_architecture_flag_records_the_style_and_tells_the_agents() {
    let dir = hexagonal_project();
    let root = dir.path();

    let init = anvil(root, &["init", "--adapter", "claude", "--architecture", "hexagonal"]);
    assert!(init.status.success(), "{}", describe(&init));
    assert!(stdout(&init).contains("hexagonal in this repository"), "{}", stdout(&init));

    let config: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".harness/config.json")).expect("config"),
    )
    .expect("json");
    assert_eq!(config["architecture"]["style"], "hexagonal");
    let claude = fs::read_to_string(root.join("CLAUDE.md")).expect("CLAUDE.md");
    assert!(
        claude.contains("Architecture: hexagonal (ports & adapters) in this repository"),
        "{claude}"
    );
}

#[test]
fn init_without_a_prompt_only_suggests_the_detected_style() {
    let dir = hexagonal_project();
    let root = dir.path();

    let init = anvil(root, &["init", "--adapter", "claude"]);
    assert!(init.status.success(), "{}", describe(&init));
    let text = stdout(&init);
    assert!(text.contains("anvil architecture set hexagonal"), "{text}");

    let config = fs::read_to_string(root.join(".harness/config.json")).expect("config");
    assert!(
        !config.contains("\"architecture\""),
        "a guessed style must not be enforced silently: {config}"
    );
}

/// Runs `anvil init` with the prompts forced on (no TTY in tests) and `stdin_data` as the answers.
fn init_prompted(root: &Path, stdin_data: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_anvil"))
        .arg("--root")
        .arg(root)
        .args(["init", "--adapter", "claude"])
        .env("NO_COLOR", "1")
        .env("ANVIL_NO_AUTO_DAEMON", "1")
        .env("ANVIL_LANG", "en")
        .env("ANVIL_INIT_FORCE_PROMPT", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn anvil");
    child.stdin.take().expect("stdin").write_all(stdin_data.as_bytes()).expect("answers");
    child.wait_with_output().expect("wait")
}

#[test]
fn interactive_init_in_an_empty_folder_offers_the_architecture_choice() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    // Answers: SonarQube "n", then architecture 1 (hexagonal).
    let init = init_prompted(root, "n\n1\n");
    assert!(init.status.success(), "{}", describe(&init));
    let text = stdout(&init);
    assert!(text.contains("Which architecture should Anvil enforce?"), "{text}");
    assert!(text.contains("rules apply once code exists there"), "{text}");

    let config: Value = serde_json::from_str(
        &fs::read_to_string(root.join(".harness/config.json")).expect("config"),
    )
    .expect("json");
    assert_eq!(config["architecture"]["style"], "hexagonal");
}

#[test]
fn interactive_init_keeps_none_when_the_answer_is_empty() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    let init = init_prompted(root, "n\n\n");
    assert!(init.status.success(), "{}", describe(&init));
    let text = stdout(&init);
    assert!(text.contains("Which architecture should Anvil enforce?"), "{text}");
    let config = fs::read_to_string(root.join(".harness/config.json")).expect("config");
    assert!(!config.contains("\"style\": \"hexagonal\""), "{config}");
}
