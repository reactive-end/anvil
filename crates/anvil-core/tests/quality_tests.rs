use std::fs;
use std::path::Path;

use anvil_core::stats::{NewFinding, list_reviews, load_stats, log_review, record_check_outcome, scope_key};
use tempfile::TempDir;

fn finding(category: &str) -> NewFinding {
    NewFinding {
        description: format!("problem in {category}"),
        category: category.to_owned(),
        severity: "medium".to_owned(),
        source: "orchestrator".to_owned(),
        file: Some("src/a.rs:10".to_owned()),
    }
}

fn check(root: &Path, passed: bool) {
    record_check_outcome(root, passed, "Tier1", &["src/a.rs".to_owned()]).expect("record check");
}

#[test]
fn check_outcomes_are_counted_for_passes_and_failures() {
    let repo = TempDir::new().expect("tempdir");
    check(repo.path(), true);
    check(repo.path(), true);
    check(repo.path(), false);
    let quality = load_stats(repo.path()).expect("stats").quality;
    assert_eq!((quality.checks_passed, quality.checks_failed), (2, 1));
    assert_eq!(quality.last_check_passed, Some(false));
}

#[test]
fn a_finding_after_a_green_check_is_after_green_and_after_a_red_one_is_not() {
    let repo = TempDir::new().expect("tempdir");
    assert!(!log_review(repo.path(), finding("dead-code")).expect("no check yet").entry.after_green);
    check(repo.path(), true);
    assert!(log_review(repo.path(), finding("dead-code")).expect("after green").entry.after_green);
    check(repo.path(), false);
    assert!(!log_review(repo.path(), finding("dead-code")).expect("after red").entry.after_green);
    let quality = load_stats(repo.path()).expect("stats").quality;
    assert_eq!((quality.findings, quality.findings_after_green), (3, 1));
}

#[test]
fn escape_rate_top_categories_and_the_ratchet_threshold() {
    let repo = TempDir::new().expect("tempdir");
    check(repo.path(), true);
    check(repo.path(), true);
    for _ in 0..3 {
        log_review(repo.path(), finding("error-handling")).expect("log");
    }
    for _ in 0..2 {
        log_review(repo.path(), finding("naming")).expect("log");
    }
    let quality = load_stats(repo.path()).expect("stats").quality;
    assert_eq!(quality.escape_rate(), Some(2.5));
    assert_eq!(quality.top_categories(5), vec![("error-handling", 3), ("naming", 2)]);
    assert_eq!(quality.ratchet_candidates(), vec![("error-handling", 3)]);
}

#[test]
fn categories_must_be_kebab_case_and_new_ones_are_reported() {
    let repo = TempDir::new().expect("tempdir");
    for bad in ["Dead Code", "dead_code", "-x", "a--b", ""] {
        assert!(log_review(repo.path(), finding(bad)).is_err(), "{bad}");
    }
    let first = log_review(repo.path(), finding("dead-code")).expect("first");
    assert!(first.new_category && first.known_categories.is_empty());
    let second = log_review(repo.path(), finding("naming")).expect("second");
    assert!(second.new_category);
    assert_eq!(second.known_categories, vec!["dead-code".to_owned()]);
    assert!(!log_review(repo.path(), finding("naming")).expect("third").new_category);
    let mut bad_severity = finding("naming");
    bad_severity.severity = "urgent".to_owned();
    assert!(log_review(repo.path(), bad_severity).is_err());
}

#[test]
fn listed_reviews_keep_their_description_and_respect_since() {
    let repo = TempDir::new().expect("tempdir");
    log_review(repo.path(), finding("dead-code")).expect("log");
    let all = list_reviews(repo.path(), None).expect("list");
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].description, "problem in dead-code");
    assert_eq!(list_reviews(repo.path(), Some(1)).expect("recent").len(), 1);
}

#[test]
fn old_stats_files_and_jsonl_lines_still_load() {
    let repo = TempDir::new().expect("tempdir");
    let state = repo.path().join(".harness/.state");
    fs::create_dir_all(&state).expect("mkdir");
    fs::write(state.join("stats.json"), r#"{"hook_calls": 4, "check_cache_hits": 2}"#).expect("stats.json");
    let lines = [
        r#"{"HookCall":{"elapsed_ms":7}}"#,
        r#""CheckCacheHit""#,
        r#"{"TokenGuard":{"blocked":true}}"#,
        r#"{"CheckOutcome":{"passed":true}}"#,
        r#"{"ReviewFinding":{"category":"naming"}}"#,
    ];
    fs::write(state.join("stats.jsonl"), lines.join("\n")).expect("stats.jsonl");
    let stats = load_stats(repo.path()).expect("load");
    assert_eq!((stats.hook_calls, stats.check_cache_hits, stats.token_guard_blocks), (5, 3, 1));
    assert_eq!((stats.quality.checks_passed, stats.quality.findings), (1, 1));
}

#[test]
fn the_scope_key_follows_the_content_of_the_changed_files() {
    let repo = TempDir::new().expect("tempdir");
    fs::write(repo.path().join("a.rs"), "one").expect("write");
    let paths = ["a.rs".to_owned(), "gone.rs".to_owned()];
    let first = scope_key(repo.path(), &paths);
    assert_eq!(first, scope_key(repo.path(), &paths));
    fs::write(repo.path().join("a.rs"), "two").expect("rewrite");
    assert_ne!(first, scope_key(repo.path(), &paths));
}
