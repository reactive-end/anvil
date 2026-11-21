//! Unit and integration tests for `anvil-tasks` ledger, identity, builder, and merge driver.

use std::fs;
use std::path::Path;
use std::process::Command;

use anvil_tasks::identity::{identity_cache_path, resolve_author, write_identity_cache};
use anvil_tasks::ledger::{
    append_entry, append_entry_as, list_tasks, start_task, start_task_as, write_task_plan,
};
use anvil_tasks::merge_driver::{install_git_merge_driver, resolve_merge};
use anvil_tasks::{ConsolidatedTasksFile, TaskStatus, build_root_tasks_json};
use tempfile::TempDir;

fn init_git_repo(dir: &Path, user_name: &str) {
    run_git(dir, &["init", "-b", "main"]);
    run_git(dir, &["config", "user.name", user_name]);
    run_git(dir, &["config", "user.email", "dev@example.com"]);
}

fn run_git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git should execute");
    assert!(
        out.status.success(),
        "git {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn identity_cache_and_git_fallback_work() {
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path(), "alice-dev");

    // Pre-seeded cache returns immediately.
    write_identity_cache(tmp.path(), "cached_octocat", "gh").expect("cache write");
    assert!(identity_cache_path(tmp.path()).exists());
    let resolved = resolve_author(tmp.path()).expect("resolve cached");
    assert_eq!(resolved, "cached_octocat");

    // Removing cache falls back to gh or git config (if gh absent/offline, uses git config).
    fs::remove_file(identity_cache_path(tmp.path())).expect("remove cache");
    let fallback = resolve_author(tmp.path()).expect("resolve fallback");
    assert!(!fallback.is_empty());
    assert_ne!(fallback, "unknown");
}

#[test]
fn ledger_creates_task_entries_plans_and_lists_sorted() {
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path(), "alice");
    write_identity_cache(tmp.path(), "alice", "test").expect("seed identity");

    let meta1 = start_task(tmp.path(), "Implement task ledger").expect("start task 1");
    assert!(meta1.task_id.starts_with("T-"));
    assert_eq!(meta1.created_by, "alice");

    let meta2 = start_task_as(tmp.path(), "bob", "Audit dependencies").expect("start task 2");
    assert_ne!(meta1.task_id, meta2.task_id);

    let entry = append_entry(
        tmp.path(),
        &meta1.task_id,
        TaskStatus::Done,
        "Completed ledger module and unit tests",
        &["crates\\anvil-tasks\\src\\ledger.rs".to_owned()],
    )
    .expect("append entry");
    assert_eq!(entry.author, "alice");
    assert_eq!(entry.files_touched, vec!["crates/anvil-tasks/src/ledger.rs"]);

    let plan_path = write_task_plan(tmp.path(), &meta1.task_id, "alice", "# Plan\n- Step 1\n")
        .expect("write plan");
    assert!(plan_path.ends_with("plan-alice.md"));
    assert!(plan_path.exists());

    let listed = list_tasks(tmp.path()).expect("list tasks");
    assert_eq!(listed.len(), 2);
    assert!(listed[0].task_id < listed[1].task_id);
}

#[test]
fn builder_is_deterministic_and_aggregates_contributors() {
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path(), "alice");

    let meta = start_task_as(tmp.path(), "alice", "Build consolidated tasks.json").expect("start");
    append_entry_as(
        tmp.path(),
        &meta.task_id,
        "bob",
        TaskStatus::Blocked,
        "Waiting on schema",
        &["src/model.rs".to_owned()],
    )
    .expect("bob entry");
    append_entry_as(
        tmp.path(),
        &meta.task_id,
        "alice",
        TaskStatus::Done,
        "Schema finalized and wired",
        &["src/builder.rs".to_owned()],
    )
    .expect("alice entry");

    let first = build_root_tasks_json(tmp.path()).expect("first build");
    let first_bytes = fs::read_to_string(tmp.path().join("tasks.json")).expect("read tasks.json");
    let second = build_root_tasks_json(tmp.path()).expect("second build");
    let second_bytes = fs::read_to_string(tmp.path().join("tasks.json")).expect("read tasks.json");

    assert_eq!(first, second);
    assert_eq!(first_bytes, second_bytes);
    assert_eq!(first.task_count, 1);

    let task = &first.tasks[0];
    assert_eq!(task.status, TaskStatus::Done);
    assert_eq!(task.contributors, vec!["alice", "bob"]);
    assert_eq!(task.entry_count, 3);
    assert_eq!(task.files_touched, vec!["src/builder.rs", "src/model.rs"]);
    assert!(task.summary.contains("alice:"));
    assert!(task.summary.contains("bob:"));
}

#[test]
fn install_merge_driver_and_resolve_merge_work() {
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path(), "alice");

    install_git_merge_driver(tmp.path()).expect("install driver");
    install_git_merge_driver(tmp.path()).expect("idempotent reinstall");

    let gitattrs = fs::read_to_string(tmp.path().join(".gitattributes")).expect("read gitattributes");
    assert_eq!(
        gitattrs.lines().filter(|l| l.trim() == "tasks.json merge=anvil-tasks").count(),
        1
    );
    let configured = run_git(tmp.path(), &["config", "--get", "merge.anvil-tasks.driver"]);
    assert!(configured.contains("anvil tasks merge-driver"));

    let base_repo = TempDir::new().expect("base");
    let meta = start_task_as(base_repo.path(), "alice", "Shared task").expect("start");
    let base_file = build_root_tasks_json(base_repo.path()).expect("base build");
    let base_path = tmp.path().join("base.json");
    fs::write(&base_path, serde_json::to_string_pretty(&base_file).expect("json")).expect("write");

    let ours_path = tmp.path().join("ours.json");
    let theirs_path = tmp.path().join("theirs.json");
    let mut ours = base_file.clone();
    let e_a = append_entry_as(
        base_repo.path(),
        &meta.task_id,
        "dev-a",
        TaskStatus::InProgress,
        "Dev A change",
        &["a.rs".to_owned()],
    )
    .expect("entry a");
    ours.tasks[0].entries.push(e_a);
    fs::write(&ours_path, serde_json::to_string_pretty(&ours).expect("json")).expect("write ours");

    let mut theirs = base_file;
    let e_b = append_entry_as(
        base_repo.path(),
        &meta.task_id,
        "dev-b",
        TaskStatus::Done,
        "Dev B change",
        &["b.rs".to_owned()],
    )
    .expect("entry b");
    theirs.tasks[0].entries.push(e_b);
    fs::write(&theirs_path, serde_json::to_string_pretty(&theirs).expect("json")).expect("write theirs");

    let merged = resolve_merge(&base_path, &ours_path, &theirs_path, tmp.path()).expect("merge");
    let parsed: ConsolidatedTasksFile =
        serde_json::from_str(&fs::read_to_string(&ours_path).expect("read merged")).expect("parse");
    assert_eq!(merged, parsed);
    assert_eq!(parsed.tasks[0].entries.len(), 3);
    assert_eq!(parsed.tasks[0].contributors, vec!["alice", "dev-a", "dev-b"]);
}
