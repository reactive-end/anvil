use std::fs;
use std::path::Path;
use std::process::Command;

use super::*;
use crate::curate::{load_untracked_edits, record_session_edit, untracked_edits_path};

fn repo_ignoring_test_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    Command::new("git").arg("init").arg("-q").current_dir(dir.path()).output().unwrap();
    fs::write(dir.path().join(".gitignore"), "/test/\n").unwrap();
    fs::create_dir_all(dir.path().join(".harness/.state")).unwrap();
    dir
}

#[test]
fn edits_under_gitignored_paths_are_not_recorded() {
    let repo = repo_ignoring_test_dir();
    assert!(!record_session_edit(repo.path(), "test/sandbox/a.txt").unwrap());
    assert!(load_untracked_edits(repo.path()).is_empty());
    assert!(!untracked_edits_path(repo.path()).exists());

    assert!(record_session_edit(repo.path(), "src/a.rs").unwrap());
    assert_eq!(load_untracked_edits(repo.path()), vec!["src/a.rs"]);
}

#[test]
fn ignored_entries_an_older_version_recorded_are_dropped_on_load() {
    let repo = repo_ignoring_test_dir();
    let body = r#"{"files":["src/b.rs","test/old.txt"],"first_edited_at":null}"#;
    fs::write(untracked_edits_path(repo.path()), body).unwrap();
    assert_eq!(load_untracked_edits(repo.path()), vec!["src/b.rs"]);
}

#[test]
fn only_ignored_paths_are_dropped() {
    let repo = repo_ignoring_test_dir();
    let files = vec!["a.rs".to_owned(), "test/x".to_owned(), "docs/test/y".to_owned()];
    assert_eq!(without_ignored(repo.path(), files), vec!["a.rs", "docs/test/y"]);
    assert!(is_git_ignored(repo.path(), "test/x"));
    assert!(!is_git_ignored(repo.path(), "a.rs"));
}

#[test]
fn outside_a_repository_every_path_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let files = vec!["test/x".to_owned()];
    assert_eq!(without_ignored(Path::new(dir.path()), files.clone()), files);
}
