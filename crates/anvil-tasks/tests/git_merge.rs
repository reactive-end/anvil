//! Real Git two-branch merge integration test for `anvil-tasks`.
//!
//! Uses `harness = false` so the compiled test executable can also act as the
//! real Git custom merge driver (`ANVIL_AS_MERGE_DRIVER=1`) during `git merge`.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anvil_tasks::ledger::{append_entry_as, start_task_as, write_task_plan};
use anvil_tasks::merge_driver::{install_git_merge_driver_with_cmd, resolve_merge};
use anvil_tasks::{ConsolidatedTasksFile, TaskStatus, build_root_tasks_json};
use tempfile::TempDir;

fn main() -> ExitCode {
    if env::var_os("ANVIL_AS_MERGE_DRIVER").is_some() {
        let args: Vec<String> = env::args().collect();
        if args.len() < 4 {
            eprintln!("expected <base> <current> <other>");
            return ExitCode::FAILURE;
        }
        let repo = env::var("ANVIL_MERGE_REPO").unwrap_or_else(|_| ".".to_owned());
        if let Err(err) = resolve_merge(
            Path::new(&args[1]),
            Path::new(&args[2]),
            Path::new(&args[3]),
            Path::new(&repo),
        ) {
            eprintln!("merge driver failed: {err:#}");
            return ExitCode::FAILURE;
        }
        return ExitCode::SUCCESS;
    }

    match run_real_git_merge_test() {
        Ok(()) => {
            println!("test real_git_two_branch_merge_preserves_both_authors ... ok");
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("test real_git_two_branch_merge_preserves_both_authors FAILED: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn run_real_git_merge_test() -> anyhow::Result<()> {
    let tmp = TempDir::new()?;
    let repo = tmp.path();
    setup_repo_with_driver(repo)?;

    let meta = start_task_as(repo, "lead", "Implement team ledger")?;
    build_root_tasks_json(repo)?;
    git(repo, &["add", "-A"])?;
    git(repo, &["commit", "-m", "chore: create task on main"])?;

    commit_branch_entry(
        repo,
        "dev-a",
        TaskStatus::InProgress,
        "dev-a implemented storage layer",
        "src/storage.rs",
    )?;
    git(repo, &["checkout", "main"])?;
    commit_branch_entry(
        repo,
        "dev-b",
        TaskStatus::Done,
        "dev-b finished API endpoints",
        "src/api.rs",
    )?;

    git(repo, &["merge", "--no-edit", "dev-a"])?;
    assert_merged_state(repo, &meta.task_id)
}

fn setup_repo_with_driver(repo: &Path) -> anyhow::Result<()> {
    git(repo, &["init", "-b", "main"])?;
    git(repo, &["config", "user.name", "coordinator"])?;
    git(repo, &["config", "user.email", "coord@example.com"])?;
    let exe = env::current_exe()?.to_string_lossy().replace('\\', "/");
    let repo_str = repo.to_string_lossy().replace('\\', "/");
    let driver_cmd =
        format!("ANVIL_AS_MERGE_DRIVER=1 ANVIL_MERGE_REPO=\"{repo_str}\" \"{exe}\" %O %A %B");
    install_git_merge_driver_with_cmd(repo, &driver_cmd)
}

fn commit_branch_entry(
    repo: &Path,
    branch_and_author: &str,
    status: TaskStatus,
    summary: &str,
    touched_file: &str,
) -> anyhow::Result<()> {
    git(repo, &["checkout", "-b", branch_and_author])?;
    let tasks = anvil_tasks::list_tasks(repo)?;
    let task_id = &tasks[0].task_id;
    append_entry_as(
        repo,
        task_id,
        branch_and_author,
        status,
        summary,
        &[touched_file.to_owned()],
    )?;
    let plan = format!("# {branch_and_author} Plan\n- {touched_file}\n");
    write_task_plan(repo, task_id, branch_and_author, &plan)?;
    build_root_tasks_json(repo)?;
    git(repo, &["add", "-A"])?;
    git(repo, &["commit", "-m", &format!("feat({branch_and_author}): update task")])?;
    Ok(())
}

fn assert_merged_state(repo: &Path, task_id: &str) -> anyhow::Result<()> {
    let unmerged = git(repo, &["diff", "--name-only", "--diff-filter=U"])?;
    anyhow::ensure!(unmerged.trim().is_empty(), "expected ZERO conflicts, found: {unmerged}");

    let entries_dir = PathBuf::from(repo)
        .join(".harness")
        .join("tasks")
        .join(task_id)
        .join("entries");
    let entry_files: Vec<String> = fs::read_dir(&entries_dir)?
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_owned))
        .collect();
    anyhow::ensure!(entry_files.iter().any(|n| n.starts_with("dev-a-")));
    anyhow::ensure!(entry_files.iter().any(|n| n.starts_with("dev-b-")));

    let raw_tasks = fs::read_to_string(repo.join("tasks.json"))?;
    anyhow::ensure!(!raw_tasks.contains("<<<<<<<"), "conflict marker in tasks.json");
    let consolidated: ConsolidatedTasksFile = serde_json::from_str(&raw_tasks)?;
    anyhow::ensure!(consolidated.task_count == 1);

    let task = &consolidated.tasks[0];
    anyhow::ensure!(task.entries.len() == 3, "expected lead + dev-a + dev-b entries");
    anyhow::ensure!(task.entries.iter().any(|e| e.author == "dev-a"));
    anyhow::ensure!(task.entries.iter().any(|e| e.author == "dev-b"));
    anyhow::ensure!(task.files_touched.contains(&"src/storage.rs".to_owned()));
    anyhow::ensure!(task.files_touched.contains(&"src/api.rs".to_owned()));
    Ok(())
}

fn git(repo: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("git").args(args).current_dir(repo).output()?;
    anyhow::ensure!(
        out.status.success(),
        "git {} failed:\nstdout: {}\nstderr: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
