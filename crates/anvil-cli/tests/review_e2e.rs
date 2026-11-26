//! `anvil review log` and `anvil stats` end to end in a temporary repository.
//! The home and Claude config directories point into the temp dir, never the real ones.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use tempfile::TempDir;

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

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn git(repo: &Path, args: &[&str]) {
    let out = Command::new("git").args(args).current_dir(repo).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn committed_repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    git(root, &["init"]);
    git(root, &["config", "user.name", "Test"]);
    git(root, &["config", "user.email", "test@example.com"]);
    fs::write(root.join("package.json"), r#"{"name":"sample","version":"1.0.0"}"#)
        .expect("package.json");
    fs::create_dir_all(root.join("src")).expect("src");
    fs::write(
        root.join("src/core.ts"),
        "export function compute(x: number): number {\n  return x * 2;\n}\n",
    )
    .expect("core.ts");
    assert!(anvil(root, &["init"]).status.success());
    fs::write(root.join(".gitignore"), ".harness/.state/\nanvil-home/\nclaude-config/\n")
        .expect("gitignore");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-m", "init"]);
    dir
}

fn log(repo: &Path, category: &str) -> String {
    let out = anvil(
        repo,
        &[
            "review",
            "log",
            "missing error handling",
            "--category",
            category,
            "--file",
            "src/core.ts:2",
        ],
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    stdout(&out)
}

#[test]
fn review_findings_after_green_feed_the_quality_section_and_the_ratchet_hint() {
    let repo = committed_repo();
    let root = repo.path();

    let before_any_check = log(root, "dead-code");
    assert!(before_any_check.contains("no green check before it"), "{before_any_check}");
    assert!(before_any_check.contains("new category"), "{before_any_check}");

    assert!(
        anvil(root, &["check", "--tier", "0", "--no-cache"]).status.success(),
        "clean tree must pass"
    );
    for _ in 0..3 {
        assert!(log(root, "error-handling").contains("after a green check"));
    }
    let stats = stdout(&anvil(root, &["stats"]));
    println!("{stats}");
    assert!(stats.contains("quality:"), "{stats}");
    assert!(stats.contains("green=1 red=0"), "{stats}");
    assert!(stats.contains("after_green=3"), "{stats}");
    assert!(stats.contains("consider: anvil ratchet log --rule error-handling"), "{stats}");

    let listed = stdout(&anvil(root, &["review", "list"]));
    assert_eq!(listed.lines().count(), 4, "{listed}");
    assert!(listed.contains("error-handling") && listed.contains("src/core.ts:2"), "{listed}");
}

#[test]
fn a_finding_after_a_red_check_is_not_after_green() {
    let repo = committed_repo();
    let root = repo.path();
    fs::write(root.join("src/bad.ts"), "// @ts-ignore\nexport const bad: number = 'x';\n")
        .expect("bad.ts");
    assert!(
        !anvil(root, &["check", "--tier", "0", "--no-cache"]).status.success(),
        "escape hatch must fail"
    );
    assert!(log(root, "naming").contains("no green check before it"));
}

#[test]
fn bad_categories_and_empty_stats_are_handled() {
    let repo = committed_repo();
    let root = repo.path();
    let out = anvil(root, &["review", "log", "x", "--category", "Not Kebab"]);
    assert!(!out.status.success());
    assert!(!stdout(&anvil(root, &["stats"])).contains("quality:"), "no data, no section");
}
