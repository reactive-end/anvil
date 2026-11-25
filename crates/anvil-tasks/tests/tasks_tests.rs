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
    TASKS_INDEX_SCHEMA_VERSION, TaskStatus, TasksIndexFile, build_root_tasks_json,
    ensure_task_for_prompt, load_active_task, mark_summary_recorded,
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
    assert_slim_index_shape(&first_bytes, &meta.task_id, 3, 2);
}

fn assert_slim_index_shape(raw_json: &str, task_id: &str, entries: usize, files: usize) {
    assert!(!raw_json.contains("\"entries\""));
    assert!(!raw_json.contains("\"files_touched\""));
    let index: TasksIndexFile = serde_json::from_str(raw_json).expect("parse slim index");
    assert_eq!(index.version, TASKS_INDEX_SCHEMA_VERSION);
    assert_eq!(index.tasks[0].entry_count, entries);
    assert_eq!(index.tasks[0].files_count, files);
    assert_eq!(
        index.tasks[0].progress_file,
        format!(".harness/tasks/{task_id}/progress.md")
    );
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
    let _meta = start_task_as(base_repo.path(), "alice", "Shared task").expect("start");
    let base_file = build_root_tasks_json(base_repo.path()).expect("base build");
    let base_index = TasksIndexFile::from(&base_file);
    let base_path = tmp.path().join("base.json");
    fs::write(&base_path, serde_json::to_string_pretty(&base_index).expect("json")).expect("write");

    let ours_path = tmp.path().join("ours.json");
    let theirs_path = tmp.path().join("theirs.json");
    let mut ours = base_index.clone();
    ours.tasks[0].contributors.push("dev-a".to_owned());
    ours.tasks[0].entry_count = 2;
    ours.tasks[0].files_count = 1;
    ours.tasks[0].summary = "Dev A change".to_owned();
    fs::write(&ours_path, serde_json::to_string_pretty(&ours).expect("json")).expect("write ours");

    let mut theirs = base_index;
    theirs.tasks[0].contributors.push("dev-b".to_owned());
    theirs.tasks[0].status = TaskStatus::Done;
    theirs.tasks[0].entry_count = 2;
    theirs.tasks[0].files_count = 1;
    theirs.tasks[0].summary = "Dev B change".to_owned();
    fs::write(&theirs_path, serde_json::to_string_pretty(&theirs).expect("json")).expect("write theirs");

    let merged = resolve_merge(&base_path, &ours_path, &theirs_path, tmp.path()).expect("merge");
    let parsed: TasksIndexFile =
        serde_json::from_str(&fs::read_to_string(&ours_path).expect("read merged")).expect("parse");
    assert_eq!(merged, parsed);
    assert_eq!(parsed.version, TASKS_INDEX_SCHEMA_VERSION);
    assert_eq!(parsed.tasks[0].entry_count, 3);
    assert_eq!(parsed.tasks[0].files_count, 2);
    assert_eq!(parsed.tasks[0].contributors, vec!["alice", "dev-a", "dev-b"]);
}

#[test]
fn resolve_merge_accepts_legacy_v1_and_outputs_v2_index() {
    let tmp = TempDir::new().expect("tempdir");
    let base_repo = TempDir::new().expect("base");
    let meta = start_task_as(base_repo.path(), "alice", "Legacy v1 upgrade").expect("start");
    let base_file = build_root_tasks_json(base_repo.path()).expect("base");

    let base_path = tmp.path().join("base.json");
    let ours_path = tmp.path().join("ours.json");
    let theirs_path = tmp.path().join("theirs.json");
    fs::write(&base_path, serde_json::to_string_pretty(&base_file).expect("json")).expect("write");

    let mut ours = base_file.clone();
    let e_a = append_entry_as(
        base_repo.path(),
        &meta.task_id,
        "dev-a",
        TaskStatus::InProgress,
        "Dev A v1",
        &["a.rs".to_owned()],
    )
    .expect("entry a");
    ours.tasks[0].entries.push(e_a);
    ours.tasks[0].contributors.push("dev-a".to_owned());
    ours.tasks[0].entry_count = 2;
    ours.tasks[0].files_touched.push("a.rs".to_owned());
    fs::write(&ours_path, serde_json::to_string_pretty(&ours).expect("json")).expect("write ours");

    let mut theirs = base_file;
    let e_b = append_entry_as(
        base_repo.path(),
        &meta.task_id,
        "dev-b",
        TaskStatus::Done,
        "Dev B v1",
        &["b.rs".to_owned()],
    )
    .expect("entry b");
    theirs.tasks[0].entries.push(e_b);
    theirs.tasks[0].contributors.push("dev-b".to_owned());
    theirs.tasks[0].entry_count = 2;
    theirs.tasks[0].files_touched.push("b.rs".to_owned());
    fs::write(&theirs_path, serde_json::to_string_pretty(&theirs).expect("json")).expect("write theirs");

    let merged = resolve_merge(&base_path, &ours_path, &theirs_path, tmp.path()).expect("merge");
    let raw = fs::read_to_string(&ours_path).expect("read merged");
    assert!(!raw.contains("\"entries\""));
    assert!(!raw.contains("\"files_touched\""));
    assert_eq!(merged.version, TASKS_INDEX_SCHEMA_VERSION);
    assert_eq!(merged.tasks[0].entry_count, 3);
    assert_eq!(merged.tasks[0].files_count, 2);
    assert_eq!(merged.tasks[0].contributors, vec!["alice", "dev-a", "dev-b"]);
}

#[test]
fn active_task_auto_lifecycle_creates_reuses_and_advances_after_done() {
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path(), "alice");
    write_identity_cache(tmp.path(), "alice", "test").expect("seed identity");

    for ignored in ["", "   ", "/compact", "/plan auth"] {
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
fn llm_controls_task_titles_with_structural_hygiene_only() {
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path(), "alice");
    write_identity_cache(tmp.path(), "alice", "test").expect("seed identity");

    assert!(anvil_tasks::validate_curated_task_title("").is_err());
    assert!(anvil_tasks::validate_curated_task_title("Line 1\nLine 2").is_err());
    assert_eq!(
        anvil_tasks::validate_curated_task_title("Fix UI?").expect("short/question title allowed"),
        "Fix UI?"
    );
    let (meta, entry) = anvil_tasks::record_curated_task(
        tmp.path(),
        "Como vamos?",
        "Concise summary decided by LLM",
        TaskStatus::Done,
        &["src/ui.ts".to_owned()],
        Some("alice"),
    )
    .expect("LLM title allowed");
    assert_eq!(meta.request, "Como vamos?");
    assert_eq!(entry.status, TaskStatus::Done);
}

#[test]
fn progress_md_and_resume_brief_track_and_recover_work() {
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path(), "alice");
    write_identity_cache(tmp.path(), "alice", "test").expect("seed identity");

    let json_raw = r#"{"project":"core","features":[{"id":"F001","name":"Implementar alertas","behavior":"Detectar bajo stock","verify":"node --test","status":"not_started"}]}"#;
    let list = anvil_tasks::parse_feature_list_json(json_raw).expect("parse list");
    anvil_tasks::import_feature_list_in_group(tmp.path(), &list, Some("core"), Some("alice"), false)
        .expect("import queued");

    let (started, _) =
        anvil_tasks::begin_existing_task(tmp.path(), "core/F001", "alice", Some("Analyze ledger"))
            .expect("begin existing");
    assert_eq!(started.status, TaskStatus::InProgress);
    let progress_path = tmp.path().join(".harness/tasks/core/F001/progress.md");
    assert!(progress_path.is_file());

    let (updated, _) = anvil_tasks::record_task_progress(
        tmp.path(),
        "core/F001",
        "alice",
        anvil_tasks::ProgressUpdate {
            status: TaskStatus::InProgress,
            summary: "Added low-stock detector",
            done_steps: &["Added detector helper".to_owned()],
            next_steps: &["Add summary report".to_owned(), "Run node --test".to_owned()],
            files: &["src/inventory.mjs".to_owned()],
            ..anvil_tasks::ProgressUpdate::default()
        },
    )
    .expect("record progress");
    assert_eq!(updated.status, TaskStatus::InProgress);
    assert!(fs::read_to_string(&progress_path).expect("read").contains("Add summary report"));

    anvil_tasks::record_active_command_outcome(tmp.path(), "node --test", false).expect("cmd");
    let brief = anvil_tasks::build_resume_brief(tmp.path(), None).expect("brief").expect("some");
    assert_eq!(brief.task_id, "core/F001");
    assert_eq!(brief.done_steps, vec!["Added detector helper"]);
    assert_eq!(brief.next_steps, vec!["Add summary report", "Run node --test"]);
    assert_eq!(brief.last_command_passed, Some(false));

    let done = anvil_tasks::ProgressUpdate {
        status: TaskStatus::Done,
        summary: "Completed feature and tests",
        evidence: Some("node --test PASS"),
        files: &["src/inventory.mjs".to_owned()],
        ..anvil_tasks::ProgressUpdate::default()
    };
    let (finished, _) =
        anvil_tasks::record_task_progress(tmp.path(), "core/F001", "alice", done).expect("done");
    assert_eq!(finished.status, TaskStatus::Done);
    assert!(anvil_tasks::build_resume_brief(tmp.path(), None).expect("ok").is_none());

    let skipped = anvil_tasks::record_task_skip(tmp.path(), "Minor typo adjustment").expect("skip");
    assert!(skipped.skipped);
    assert_eq!(list_tasks(tmp.path()).expect("list").len(), 1);
}

#[test]
fn task_skip_persists_across_edits_until_explicit_clear() {
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path(), "alice");
    write_identity_cache(tmp.path(), "alice", "test").expect("seed identity");

    let skipped = anvil_tasks::record_task_skip(tmp.path(), "Minor typo adjustment").expect("skip");
    assert!(skipped.skipped);
    assert!(
        !anvil_tasks::record_session_edit(tmp.path(), "src/minor.ts").expect("skip holds"),
        "active skipped turn must suppress edit nagging"
    );
    assert!(anvil_tasks::load_untracked_edits(tmp.path()).is_empty());
    anvil_tasks::clear_skipped_active_task(tmp.path());
    assert!(anvil_tasks::load_active_task(tmp.path()).expect("ok").is_none());
}

#[test]
fn short_sequential_ids_custom_id_natural_sort_and_migration() {
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path(), "alice");
    write_identity_cache(tmp.path(), "alice", "test").expect("seed identity");

    let m1 = start_task_as(tmp.path(), "alice", "First short task").expect("m1");
    let m2 = start_task_as(tmp.path(), "bob", "Second short task").expect("m2");
    assert_eq!(m1.task_id, "T-001");
    assert_eq!(m2.task_id, "T-002");

    let custom = anvil_tasks::start_task_with_id(tmp.path(), "alice", "Custom code task", Some("F039"))
        .expect("custom id");
    assert_eq!(custom.task_id, "F039");
    assert!(
        anvil_tasks::start_task_with_id(tmp.path(), "alice", "Duplicate code", Some("F039")).is_err()
    );

    assert_eq!(anvil_tasks::compare_task_ids("T-002", "T-010"), std::cmp::Ordering::Less);
    assert_eq!(anvil_tasks::compare_task_ids("F002", "F038"), std::cmp::Ordering::Less);

    // Seed a legacy `T-YYYYMMDD-<ULID>` folder and migrate it chronologically to `T-003`.
    let legacy_id = "T-20261121-01M7YC7MM5WX2CGP";
    let legacy_dir = tmp.path().join(".harness/tasks").join(legacy_id);
    std::fs::create_dir_all(legacy_dir.join("entries")).expect("legacy dir");
    std::fs::write(
        legacy_dir.join("meta.json"),
        format!(
            r#"{{"task_id":"{legacy_id}","created_by":"alice","request":"Legacy task","created_at":"2026-11-21T00:00:00Z"}}"#
        ),
    )
    .expect("legacy meta");
    let moved = anvil_tasks::migrate_legacy_task_ids(tmp.path()).expect("migrate");
    assert_eq!(moved, vec![(legacy_id.to_owned(), "T-003".to_owned())]);
    assert!(!legacy_dir.exists());
    assert!(tmp.path().join(".harness/tasks/T-003/meta.json").is_file());
    assert!(anvil_tasks::migrate_legacy_task_ids(tmp.path()).expect("idempotent").is_empty());
}
