//! `anvil grep` end to end in a temporary repository: exit codes, JSON shape, flags, symbol annotation.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

const ORDERS: &str = "export class OrdersService {\n  findOrder(id: string) {\n    const needle = id;\n    return needle;\n  }\n}\n\nexport function helper() {\n  return 'needle';\n}\n";

fn anvil(repo: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_anvil"))
        .arg("--root")
        .arg(repo)
        .args(args)
        .env("ANVIL_HOME", repo.join("anvil-home"))
        .env("CLAUDE_CONFIG_DIR", repo.join("claude-config"))
        .env("NO_COLOR", "1")
        .output()
        .expect("run anvil")
}

fn grep(repo: &Path, args: &[&str]) -> Output {
    let mut full = vec!["grep"];
    full.extend_from_slice(args);
    full.extend_from_slice(&["--backend", "builtin"]);
    anvil(repo, &full)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, content).expect("write");
}

fn repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    write(dir.path(), "src/orders.ts", ORDERS);
    write(dir.path(), "notes/readme.md", "A Needle in the notes\nsecond line\n");
    write(dir.path(), ".github/workflows/ci.yml", "run: needle\n");
    write(dir.path(), ".hidden/secret.txt", "needle\n");
    dir
}

#[test]
fn exit_codes_follow_the_ripgrep_convention() {
    let dir = repo();
    let root = dir.path();

    let hit = grep(root, &["needle"]);
    assert_eq!(hit.status.code(), Some(0), "{}", String::from_utf8_lossy(&hit.stderr));
    let text = stdout(&hit);
    assert!(text.contains("src/orders.ts\n  3: const needle = id;"), "{text}");
    assert!(text.contains("# grep: "), "{text}");

    let miss = grep(root, &["no_such_token_anywhere"]);
    assert_eq!(miss.status.code(), Some(1));
    assert!(stdout(&miss).contains("no matches"), "{}", stdout(&miss));

    let bad = grep(root, &["("]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("invalid grep pattern"));

    let unknown = anvil(root, &["grep", "needle", "--backend", "bogus"]);
    assert_eq!(unknown.status.code(), Some(2));
    let missing_path = grep(root, &["needle", "no/such/dir"]);
    assert_eq!(missing_path.status.code(), Some(2));
}

fn json_report(repo: &Path, args: &[&str]) -> Value {
    let out = grep(repo, args);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_str(&stdout(&out)).expect("json report")
}

#[test]
fn json_output_has_the_report_shape() {
    let dir = repo();
    let report = json_report(dir.path(), &["needle", "--json"]);

    assert_eq!(report["backend"], "builtin");
    assert_eq!(report["truncated"], false);
    assert_eq!(report["total_matches"], 5);
    assert_eq!(report["files_with_matches"], 3);
    assert!(report["files_searched"].as_u64().expect("searched") >= 3);
    assert!(report["note"].is_null());
    let files = report["files"].as_array().expect("files");
    let paths: Vec<&str> = files.iter().map(|f| f["path"].as_str().expect("path")).collect();
    assert_eq!(paths, [".github/workflows/ci.yml", "notes/readme.md", "src/orders.ts"]);
    assert_eq!(files[2]["total"], 3);
}

#[test]
fn json_matches_carry_context_and_a_null_symbol_without_a_graph() {
    let dir = repo();
    let report = json_report(dir.path(), &["needle", "src", "--json", "-C", "1"]);
    let matches = &report["files"][0]["matches"];

    assert_eq!(matches[0]["line"], 3);
    assert_eq!(matches[0]["text"], "const needle = id;");
    assert_eq!(matches[0]["before"][0][0], 2);
    assert_eq!(matches[0]["after"].as_array().map(Vec::len), Some(0), "line 4 is a match");
    assert_eq!(matches[1]["line"], 4);
    assert_eq!(matches[1]["after"][0][1], "}");
    assert!(matches[0]["symbol"].is_null(), "no graph yet, so no annotation");
}

#[test]
fn matches_carry_their_enclosing_symbol_once_a_graph_exists() {
    let dir = repo();
    let root = dir.path();
    let before = stdout(&grep(root, &["needle", "src"]));
    assert!(!before.contains('['), "no brackets without a graph: {before}");

    let build = anvil(root, &["graph", "build"]);
    assert!(build.status.success(), "{}", String::from_utf8_lossy(&build.stderr));

    let after = stdout(&grep(root, &["needle", "src"]));
    assert!(after.contains("const needle = id;  [method findOrder]"), "{after}");
    assert!(after.contains("return 'needle';  [fn helper]"), "{after}");

    let json: Value =
        serde_json::from_str(&stdout(&grep(root, &["needle", "src", "--json"]))).expect("json");
    assert_eq!(json["files"][0]["matches"][1]["symbol"], "method findOrder");
}

#[test]
fn flags_shape_the_search() {
    let dir = repo();
    let root = dir.path();

    let smart = stdout(&grep(root, &["needle", "notes"]));
    assert!(smart.contains("1: A Needle in the notes"), "smart case matches Needle: {smart}");
    let exact = grep(root, &["needle", "notes", "--case-sensitive"]);
    assert_eq!(exact.status.code(), Some(1), "case-sensitive lowercase misses Needle");
    let upper = grep(root, &["NEEDLE", "notes", "-i"]);
    assert_eq!(upper.status.code(), Some(0));
}

#[test]
fn matcher_flags_fixed_strings_and_whole_words() {
    let dir = repo();
    let root = dir.path();
    let fixed = grep(root, &["c.nst", "src", "-F"]);
    assert_eq!(fixed.status.code(), Some(1));
    assert_eq!(grep(root, &["c.nst", "src"]).status.code(), Some(0));
    assert_eq!(grep(root, &["need", "src", "-w"]).status.code(), Some(1));
}

#[test]
fn globs_listing_hidden_and_caps_shape_the_output() {
    let dir = repo();
    let root = dir.path();
    let globbed = stdout(&grep(root, &["needle", "-g", "*.md"]));
    assert!(globbed.contains("notes/readme.md") && !globbed.contains("src/orders.ts"), "{globbed}");

    let files = stdout(&grep(root, &["needle", "-l"]));
    assert!(
        files.starts_with(".github/workflows/ci.yml (1)\nnotes/readme.md (1)\nsrc/orders.ts (3)"),
        "{files}"
    );

    let hidden_off = stdout(&grep(root, &["needle", "-l"]));
    assert!(!hidden_off.contains(".hidden"), "{hidden_off}");
    let hidden_on = stdout(&grep(root, &["needle", "-l", "--hidden"]));
    assert!(hidden_on.contains(".hidden/secret.txt (1)"), "{hidden_on}");

    let capped = stdout(&grep(root, &["needle", "--max-matches", "2"]));
    assert!(capped.contains("2 of 5 match(es)") && capped.contains("truncated"), "{capped}");
    let per_file = stdout(&grep(root, &["needle", "src", "--max-per-file", "1"]));
    assert!(per_file.contains("(+2 more in this file)"), "{per_file}");
}

#[test]
fn config_backend_is_honoured_and_a_missing_tgrep_index_never_creates_one() {
    let dir = repo();
    let root = dir.path();
    write(root, ".harness/config.json", "{ \"search\": { \"backend\": \"builtin\" } }\n");
    let out = anvil(root, &["grep", "needle", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", String::from_utf8_lossy(&out.stderr));
    let report: Value = serde_json::from_str(&stdout(&out)).expect("json");
    assert_eq!(report["backend"], "builtin");

    let auto = anvil(root, &["grep", "needle", "--json", "--backend", "auto"]);
    let auto_report: Value = serde_json::from_str(&stdout(&auto)).expect("json");
    assert_eq!(auto_report["backend"], "builtin", "auto needs .tgrep/ and a running server");
    assert!(!root.join(".tgrep").exists());
}
