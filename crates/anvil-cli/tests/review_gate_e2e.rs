use std::fs;
use std::path::Path;
use std::process::{Command, Output};

fn anvil(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_anvil"))
        .arg("--root")
        .arg(root)
        .args(args)
        .output()
        .expect("failed to execute anvil binary")
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git").args(args).current_dir(root).output().expect("git");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn stdout_ok(out: &Output, what: &str) -> String {
    assert!(
        out.status.success(),
        "{what} failed:\n{}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_of(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn orchestrator_mode_closes_a_task_only_after_its_current_change_is_approved() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    git(root, &["init", "-q"]);
    git(root, &["config", "user.email", "gate@example.com"]);
    git(root, &["config", "user.name", "gate"]);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("src/a.ts"), "export const a = 1;\n").unwrap();
    stdout_ok(&anvil(root, &["init"]), "init");
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", "init"]);
    stdout_ok(&anvil(root, &["orchestrator", "on"]), "mode on");
    stdout_ok(&anvil(root, &["tasks", "start", "Double a"]), "start");
    fs::write(root.join("src/a.ts"), "export const a = 2;\n").unwrap();

    let refused = anvil(root, &["tasks", "done", "T-001", "doubled a"]);
    assert!(!refused.status.success(), "closing without review must fail");
    assert!(
        stderr_of(&refused).contains("review T-001 before closing it"),
        "{}",
        stderr_of(&refused)
    );
    assert!(!anvil(root, &["review", "approve", "T-001", "looked fine"]).status.success());

    let packet = stdout_ok(&anvil(root, &["review", "packet"]), "packet");
    assert!(packet.contains("Review packet · T-001 · Double a"), "{packet}");
    assert!(packet.contains("M src/a.ts  +1 -1") && packet.contains("+export const a = 2;"));
    assert!(packet.contains("Check each before approving"));

    fs::write(root.join("src/a.ts"), "export const a = 3;\n").unwrap();
    let stale = anvil(root, &["review", "approve", "T-001", "checked the constant"]);
    assert!(!stale.status.success(), "approving a stale packet must fail");
    stdout_ok(&anvil(root, &["review", "packet", "T-001"]), "fresh packet");
    stdout_ok(&anvil(root, &["review", "approve", "T-001", "checked the constant"]), "approve");

    fs::write(root.join("src/a.ts"), "export const a = 4;\n").unwrap();
    let edited = anvil(root, &["tasks", "done", "T-001", "doubled a"]);
    assert!(
        stderr_of(&edited).contains("changed after its review was approved"),
        "{}",
        stderr_of(&edited)
    );
    fs::write(root.join("src/a.ts"), "export const a = 3;\n").unwrap();
    stdout_ok(&anvil(root, &["tasks", "done", "T-001", "doubled a"]), "done after approval");

    stdout_ok(&anvil(root, &["orchestrator", "off"]), "mode off");
    stdout_ok(&anvil(root, &["tasks", "start", "Unreviewed outside the mode"]), "start 2");
    fs::write(root.join("src/a.ts"), "export const a = 5;\n").unwrap();
    stdout_ok(&anvil(root, &["tasks", "done", "T-002", "no gate when the mode is off"]), "done 2");
}
