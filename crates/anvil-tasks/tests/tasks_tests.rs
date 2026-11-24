//! Unit and integration tests for `anvil-tasks` ledger, identity, builder, and merge driver.

use std::fs;
use std::path::Path;
use std::process::Command;

use anvil_tasks::identity::{identity_cache_path, resolve_author, write_identity_cache};
use anvil_tasks::ledger::{
    append_entry, append_entry_as, list_tasks, start_task, start_task_as, write_task_plan,
};
use anvil_tasks::merge_driver::{install_git_merge_driver, resolve_merge};
use anvil_tasks::{
    ConsolidatedTasksFile, TaskStatus, build_root_tasks_json, ensure_task_for_prompt,
    load_active_task, mark_summary_recorded,
};
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

#[test]
fn active_task_auto_lifecycle_creates_reuses_and_advances_after_done() {
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path(), "alice");
    write_identity_cache(tmp.path(), "alice", "test").expect("seed identity");

    for ignored in ["", "   ", "/compact", "/plan auth", "ok", "si", "gracias", "yes"] {
        assert!(
            ensure_task_for_prompt(tmp.path(), ignored)
                .expect("ignored check")
                .is_none(),
            "expected prompt `{ignored}` to be ignored"
        );
    }

    let (first, created) = ensure_task_for_prompt(tmp.path(), "Implement automatic task tracking")
        .expect("first prompt")
        .expect("should create");
    assert!(created);
    assert_eq!(first.status, TaskStatus::InProgress);
    assert!(first.summary_recorded_at.is_none());
    assert!(tmp.path().join("tasks.json").exists());

    let (reused, created_again) =
        ensure_task_for_prompt(tmp.path(), "Also handle follow-up refinements")
            .expect("follow-up prompt")
            .expect("should reuse");
    assert!(!created_again);
    assert_eq!(reused.task_id, first.task_id);

    mark_summary_recorded(tmp.path(), &first.task_id, TaskStatus::Done).expect("mark done");
    let loaded = load_active_task(tmp.path()).expect("load").expect("present");
    assert_eq!(loaded.status, TaskStatus::Done);
    assert!(loaded.summary_recorded_at.is_some());

    let (second, created_second) =
        ensure_task_for_prompt(tmp.path(), "Refactor parser error diagnostics")
            .expect("next task prompt")
            .expect("should create new");
    assert!(created_second);
    assert_ne!(second.task_id, first.task_id);
    assert!(second.summary_recorded_at.is_none());
}

#[test]
fn same_prompt_after_done_does_not_create_duplicate_task() {
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path(), "alice");
    write_identity_cache(tmp.path(), "alice", "test").expect("seed identity");

    let prompt = "Implement automatic task tracking for Antigravity";
    let (first, created) = ensure_task_for_prompt(tmp.path(), prompt)
        .expect("initial prompt")
        .expect("should create task");
    assert!(created);

    mark_summary_recorded(tmp.path(), &first.task_id, TaskStatus::Done).expect("mark done");

    let (reused, created_again) = ensure_task_for_prompt(tmp.path(), prompt)
        .expect("repeated PreInvocation prompt")
        .expect("should return existing done task");
    assert!(!created_again);
    assert_eq!(reused.task_id, first.task_id);
    assert_eq!(reused.status, TaskStatus::Done);
    assert_eq!(list_tasks(tmp.path()).expect("list tasks").len(), 1);
}

#[test]
fn record_touched_file_accumulates_unique_project_paths() {
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path(), "alice");
    write_identity_cache(tmp.path(), "alice", "test").expect("seed identity");

    let (state, created) = ensure_task_for_prompt(tmp.path(), "Refactor authentication redirect flow")
        .expect("ensure task")
        .expect("task created");
    assert!(created);

    anvil_tasks::record_touched_file(tmp.path(), r"src\auth.kt").expect("record auth");
    anvil_tasks::record_touched_file(tmp.path(), "./src/webview.kt").expect("record webview");
    anvil_tasks::record_touched_file(tmp.path(), "src/auth.kt").expect("dedupe auth");
    anvil_tasks::record_touched_file(tmp.path(), ".harness/config.json").expect("skip harness");
    anvil_tasks::record_touched_file(tmp.path(), "tasks.json").expect("skip tasks.json");

    let active = anvil_tasks::load_active_task(tmp.path())
        .expect("load active")
        .expect("active exists");
    assert_eq!(active.task_id, state.task_id);
    assert_eq!(active.touched_files, vec!["src/auth.kt", "src/webview.kt"]);
}

#[test]
fn lazy_task_materializes_only_on_real_file_edit() {
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path(), "alice");
    write_identity_cache(tmp.path(), "alice", "test").expect("seed identity");

    let recorded = anvil_tasks::record_pending_prompt(
        tmp.path(),
        "Implement lazy edit tracking without noise",
    )
    .expect("record pending prompt");
    assert!(recorded);
    assert!(list_tasks(tmp.path()).expect("list tasks").is_empty());

    let ignored = anvil_tasks::materialize_task_on_edit(tmp.path(), ".harness/config.json")
        .expect("ignore harness path");
    assert!(ignored.is_none());
    assert!(list_tasks(tmp.path()).expect("list tasks").is_empty());

    let created = anvil_tasks::materialize_task_on_edit(tmp.path(), "src/main.rs")
        .expect("materialize on real edit")
        .expect("task created");
    assert_eq!(created.touched_files, vec!["src/main.rs"]);
    assert_eq!(list_tasks(tmp.path()).expect("list tasks").len(), 1);

    let second = anvil_tasks::materialize_task_on_edit(tmp.path(), "src/lib.rs")
        .expect("append second file");
    assert!(second.is_none());
    let active = anvil_tasks::load_active_task(tmp.path())
        .expect("load active")
        .expect("active exists");
    assert_eq!(active.touched_files, vec!["src/lib.rs", "src/main.rs"]);
}

#[test]
fn conversational_prompts_and_chatty_titles_are_strictly_filtered() {
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path(), "alice");
    write_identity_cache(tmp.path(), "alice", "test").expect("seed identity");

    for noisy in [
        "Como vamos con tal proceso",
        "¿Cómo vamos con el despliegue?",
        "Que solucionan esos 10 fix, describemelo",
        "Si me gustaría, así evitamos recordar comandos",
        "Sube estos cambios al repositorio",
        "El primero esta increible, ese sera",
        "dale sigue",
    ] {
        assert!(
            anvil_tasks::is_conversational_or_trivial_prompt(noisy),
            "expected `{noisy}` to be classified as conversational noise"
        );
        assert!(
            !anvil_tasks::record_pending_prompt(tmp.path(), noisy).expect("check pending"),
            "expected `{noisy}` not to record pending prompt"
        );
    }

    for bad_title in [
        "Como vamos con tal proceso",
        "Oye podrias reorganizar el modulo X",
        "¿Reorganización del módulo X?",
        "corto",
    ] {
        assert!(
            anvil_tasks::validate_curated_task_title(bad_title).is_err(),
            "expected `{bad_title}` to fail curated title validation"
        );
    }

    let (meta, entry) = anvil_tasks::record_curated_task(
        tmp.path(),
        "Reorganización visual del módulo X",
        "Reestructurados paneles, grillas y jerarquía tipográfica del módulo X",
        TaskStatus::Done,
        &["src/module_x.tsx".to_owned()],
        Some("alice"),
    )
    .expect("curated task");
    assert_eq!(meta.request, "Reorganización visual del módulo X");
    assert_eq!(entry.status, TaskStatus::Done);

    let skipped = anvil_tasks::record_task_skip(tmp.path(), "Minor typo adjustment")
        .expect("skip task");
    assert!(skipped.skipped);
    assert_eq!(list_tasks(tmp.path()).expect("list").len(), 1);
}
