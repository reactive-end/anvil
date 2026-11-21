//! Hardening Wave H2 tests: `resolve_author_fast` and locked `active-task.json` RMW.

use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Barrier};
use std::thread;

use anvil_tasks::active::active_task_lock_path;
use anvil_tasks::identity::identity_cache_path;
use anvil_tasks::{
    TaskStatus, ensure_task_for_prompt, list_tasks, load_active_task, mark_summary_recorded,
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
    assert!(!active_task_lock_path(tmp.path()).exists());
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
    assert!(!active_task_lock_path(tmp.path()).exists());
}
