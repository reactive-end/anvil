use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};
use tempfile::TempDir;

use anvil_core::git::{
    DiffScope, all_diff, changed_files, current_branch, file_diff, head_commit, repo_root,
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
