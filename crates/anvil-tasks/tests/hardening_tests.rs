//! Hardening Wave H2 tests: `resolve_author_fast` and locked `active-task.json` RMW.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;

use anvil_tasks::active::active_task_lock_path;
use anvil_tasks::identity::identity_cache_path;
use anvil_tasks::ledger::{start_task_as, task_dir};
use anvil_tasks::merge_driver::resolve_merge;
use anvil_tasks::{
    TASKS_INDEX_SCHEMA_VERSION, TaskStatus, TasksIndexFile, build_root_tasks_json,
    ensure_task_for_prompt, list_tasks, load_active_task, mark_summary_recorded,
    resolve_author_fast,
};
use chrono::{Duration, Utc};
use serde_json::json;
use tempfile::TempDir;

fn init_git_repo(dir: &Path, user_name: &str) {
    for args in [
        &["init", "-b", "main"][..],
        &["config", "user.name", user_name][..],
        &["config", "user.email", "dev@example.com"][..],
    ] {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .expect("git executes");
        assert!(out.status.success(), "git {:?} failed", args);
    }
}

#[test]
fn resolve_author_fast_uses_stale_cache_and_git_fallback_without_network() {
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path(), "fast-git-author");

    // 1. Without identity.json, falls back to local git config user.name.
    let from_git = resolve_author_fast(tmp.path()).expect("git fallback");
    assert_eq!(from_git, "fast-git-author");

    // 2. With a >24h stale identity.json cache, fast path still trusts the cache.
    let cache_path = identity_cache_path(tmp.path());
    fs::create_dir_all(cache_path.parent().expect("parent")).expect("mkdir");
    let stale_time = (Utc::now() - Duration::days(30)).to_rfc3339();
    let payload = json!({
        "author": "stale-gh-user",
        "source": "gh",
        "resolved_at": stale_time,
    });
    fs::write(&cache_path, payload.to_string()).expect("write stale cache");

    let from_stale_cache = resolve_author_fast(tmp.path()).expect("stale cache hit");
    assert_eq!(from_stale_cache, "stale-gh-user");

    // 3. ensure_task_for_prompt uses resolve_author_fast (picks up stale cache, not gh).
    let (state, created) = ensure_task_for_prompt(tmp.path(), "Implement H2 lock hardening")
        .expect("ensure task")
        .expect("created");
    assert!(created);
    let tasks = list_tasks(tmp.path()).expect("list tasks");
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].task_id, state.task_id);
    assert_eq!(tasks[0].created_by, "stale-gh-user");
}

#[test]
fn concurrent_ensure_task_for_prompt_serializes_via_file_lock() {
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path(), "concurrent-dev");

    let repo = Arc::new(tmp.path().to_path_buf());
    let workers = 8usize;
    let barrier = Arc::new(Barrier::new(workers));

    let mut handles = Vec::with_capacity(workers);
    for _ in 0..workers {
        let repo = Arc::clone(&repo);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            ensure_task_for_prompt(&repo, "Concurrent hook invocation for the same prompt")
                .expect("locked ensure_task_for_prompt")
                .expect("non-ignored prompt")
        }));
    }

    let results: Vec<_> = handles
        .into_iter()
        .map(|h| h.join().expect("thread joined"))
        .collect();

    let created_count = results.iter().filter(|(_, created)| *created).count();
    assert_eq!(created_count, 1, "exactly one worker should create the task");

    let first_id = &results[0].0.task_id;
    assert!(results.iter().all(|(s, _)| &s.task_id == first_id));
    assert_eq!(list_tasks(tmp.path()).expect("list").len(), 1);
    anvil_core::fs_util::with_file_lock(&active_task_lock_path(tmp.path()), 50, || Ok(()))
        .expect("active-task lock released");
}

#[test]
fn concurrent_mark_summary_recorded_never_corrupts_active_task() {
    let tmp = TempDir::new().expect("tempdir");
    init_git_repo(tmp.path(), "summary-dev");

    let (state, created) = ensure_task_for_prompt(tmp.path(), "Concurrent summary update test")
        .expect("start")
        .expect("created");
    assert!(created);

    let repo = Arc::new(tmp.path().to_path_buf());
    let task_id = Arc::new(state.task_id);
    let workers = 6usize;
    let barrier = Arc::new(Barrier::new(workers));

    let handles: Vec<_> = (0..workers)
        .map(|i| {
            let repo = Arc::clone(&repo);
            let task_id = Arc::clone(&task_id);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                barrier.wait();
                let status = if i % 2 == 0 {
                    TaskStatus::Done
                } else {
                    TaskStatus::InProgress
                };
                mark_summary_recorded(&repo, &task_id, status).expect("locked mark summary");
            })
        })
        .collect();

    for h in handles {
        h.join().expect("join");
    }

    let final_state = load_active_task(tmp.path())
        .expect("load active")
        .expect("present");
    assert_eq!(final_state.task_id, *task_id);
    assert!(final_state.summary_recorded_at.is_some());
    anvil_core::fs_util::with_file_lock(&active_task_lock_path(tmp.path()), 50, || Ok(()))
        .expect("active-task lock released");
}

#[test]
fn resolve_merge_never_resurrects_deleted_tasks() {
    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path();

    let kept = start_task_as(repo, "alice", "Keep this task").expect("start kept");
    let deleted = start_task_as(repo, "alice", "Delete this task").expect("start deleted");
    let base_file = build_root_tasks_json(repo).expect("build base");
    assert_eq!(base_file.task_count, 2);

    let base_path = repo.join("base.json");
    let ours_path = repo.join("ours.json");
    let theirs_path = repo.join("theirs.json");
    let base_index = TasksIndexFile::from(&base_file);
    let base_json = serde_json::to_string_pretty(&base_index).expect("serialize base");
    fs::write(&base_path, &base_json).expect("write base");
    fs::write(&ours_path, &base_json).expect("write stale ours");

    fs::remove_dir_all(task_dir(repo, &deleted.task_id)).expect("delete task from .harness/tasks");
    let after_delete = build_root_tasks_json(repo).expect("rebuild after delete");
    let after_index = TasksIndexFile::from(&after_delete);
    fs::write(
        &theirs_path,
        serde_json::to_string_pretty(&after_index).expect("serialize theirs"),
    )
    .expect("write theirs");

    let merged = resolve_merge(&base_path, &ours_path, &theirs_path, repo).expect("resolve_merge");
    let parsed: TasksIndexFile =
        serde_json::from_str(&fs::read_to_string(&ours_path).expect("read merged"))
            .expect("parse merged");
    assert_eq!(merged, parsed);
    assert_eq!(merged.task_count, 1);
    assert_eq!(merged.tasks[0].task_id, kept.task_id);

    // Fallback when `.harness/tasks/` is absent must also ignore `%O` (`base`) so tasks
    // deleted from both `ours` and `theirs` are never resurrected.
    let empty_repo = TempDir::new().expect("empty repo");
    let empty_json = serde_json::to_string_pretty(&TasksIndexFile {
        version: TASKS_INDEX_SCHEMA_VERSION,
        task_count: 0,
        updated_at: None,
        tasks: Vec::new(),
    })
    .expect("serialize empty");
    let empty_ours = empty_repo.path().join("ours.json");
    let empty_theirs = empty_repo.path().join("theirs.json");
    fs::write(&empty_ours, &empty_json).expect("write empty ours");
    fs::write(&empty_theirs, &empty_json).expect("write empty theirs");

    let fallback = resolve_merge(&base_path, &empty_ours, &empty_theirs, empty_repo.path())
        .expect("fallback merge");
    assert_eq!(fallback.task_count, 0);
}
