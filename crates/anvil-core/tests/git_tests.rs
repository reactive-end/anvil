use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};
use tempfile::TempDir;

use anvil_core::git::{
    DiffScope, all_diff, batched_working_tree_diffs, changed_files, current_branch, file_diff,
    head_commit, repo_root, working_tree_status,
};

fn git(cwd: &Path, args: &[&str]) -> Result<()> {
    let out = Command::new("git").args(args).current_dir(cwd).output()?;
    if !out.status.success() {
        bail!("git failed: {}", String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

#[test]
fn git_helpers_inspect_repo_branch_commit_and_diffs() -> Result<()> {
    let tmp = TempDir::new()?;
    let dir = tmp.path();
    git(dir, &["init", "-b", "main"])?;
    git(dir, &["config", "user.name", "Anvil Test"])?;
    git(dir, &["config", "user.email", "test@anvil.local"])?;

    fs::write(dir.join("a.txt"), "hello\n")?;
    git(dir, &["add", "a.txt"])?;
    git(dir, &["commit", "-m", "init"])?;

    assert!(repo_root(dir)?.exists());
    assert_eq!(current_branch(dir)?, "main");
    assert_eq!(head_commit(dir)?.len(), 40);

    fs::write(dir.join("a.txt"), "hello world\n")?;
    fs::write(dir.join("b.txt"), "untracked\n")?;
    assert_eq!(changed_files(dir, DiffScope::Unstaged)?, vec!["a.txt"]);
    assert_eq!(
        changed_files(dir, DiffScope::WorkingTree)?,
        vec!["a.txt", "b.txt"]
    );
    assert!(file_diff(dir, "a.txt", false)?.contains("+hello world"));

    git(dir, &["add", "a.txt"])?;
    assert_eq!(changed_files(dir, DiffScope::Staged)?, vec!["a.txt"]);
    assert!(all_diff(dir, true)?.contains("+hello world"));
    assert_eq!(changed_files(dir, DiffScope::Head)?, vec!["a.txt", "b.txt"]);
    Ok(())
}

#[test]
fn pure_rust_repo_root_walks_subdirs_and_worktree_git_files() -> Result<()> {
    let tmp = TempDir::new()?;
    let main_repo = tmp.path().join("main-repo");
    let nested = main_repo.join("crates").join("core").join("src");
    fs::create_dir_all(main_repo.join(".git"))?;
    fs::create_dir_all(&nested)?;

    assert_eq!(repo_root(&nested)?, main_repo);

    let wt = tmp.path().join("worktree-1");
    let wt_sub = wt.join("src");
    fs::create_dir_all(&wt_sub)?;
    fs::write(wt.join(".git"), "gitdir: ../main-repo/.git/worktrees/worktree-1\n")?;
    assert_eq!(repo_root(&wt_sub)?, wt);

    let outside = tmp.path().join("not-a-repo");
    fs::create_dir_all(&outside)?;
    assert!(repo_root(&outside).is_err());
    Ok(())
}

#[test]
fn working_tree_status_and_batched_diffs_handle_spaces_and_non_ascii() -> Result<()> {
    let tmp = TempDir::new()?;
    let dir = tmp.path();
    git(dir, &["init", "-b", "main"])?;
    git(dir, &["config", "user.name", "Anvil Test"])?;
    git(dir, &["config", "user.email", "test@anvil.local"])?;

    fs::create_dir_all(dir.join("dir with spaces"))?;
    fs::write(dir.join("dir with spaces/café file.txt"), "v1\n")?;
    fs::write(dir.join("to_delete.txt"), "bye\n")?;
    git(dir, &["add", "."])?;
    git(dir, &["commit", "-m", "init"])?;

    fs::write(dir.join("dir with spaces/café file.txt"), "v2\n")?;
    fs::remove_file(dir.join("to_delete.txt"))?;
    fs::write(dir.join("added.txt"), "new\n")?;
    git(dir, &["add", "added.txt"])?;
    fs::write(dir.join("untracked space.txt"), "u\n")?;

    let status = working_tree_status(dir)?;
    assert_eq!(status.added, vec!["added.txt"]);
    assert_eq!(status.modified, vec!["dir with spaces/café file.txt"]);
    assert_eq!(status.deleted, vec!["to_delete.txt"]);
    assert_eq!(status.untracked, vec!["untracked space.txt"]);

    let all = changed_files(dir, DiffScope::WorkingTree)?;
    assert_eq!(
        all,
        vec![
            "added.txt",
            "dir with spaces/café file.txt",
            "to_delete.txt",
            "untracked space.txt",
        ]
    );

    let diffs = batched_working_tree_diffs(dir)?;
    assert!(
        diffs
            .get("dir with spaces/café file.txt")
            .is_some_and(|d| d.contains("+v2"))
    );
    assert!(diffs.get("added.txt").is_some_and(|d| d.contains("+new")));
    assert!(diffs.get("to_delete.txt").is_some_and(|d| d.contains("-bye")));
    Ok(())
}
