//! Real Git two-branch merge integration test for `anvil-tasks`.
//!
//! Uses `harness = false` so the compiled test executable can also act as the
//! real Git custom merge driver (`ANVIL_AS_MERGE_DRIVER=1`) during `git merge`.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anvil_tasks::ledger::{append_entry_as, start_task_as, write_task_plan};
use anvil_tasks::merge_driver::{
    DEFAULT_DRIVER_CMD, install_git_merge_driver, install_git_merge_driver_with_cmd, resolve_merge,
};
use anvil_tasks::{TasksIndexFile, TaskStatus, build_root_tasks_json, read_single_task};
use tempfile::TempDir;

type TestCase = (&'static str, fn() -> anyhow::Result<()>);

fn main() -> ExitCode {
    if env::var_os("ANVIL_AS_MERGE_DRIVER").is_some() {
        return run_as_merge_driver();
    }

    let tests: [TestCase; 2] = [
        (
            "real_git_two_branch_merge_preserves_both_authors",
            run_real_git_merge_test,
        ),
        (
            "missing_anvil_falls_back_to_git_merge_file",
            run_missing_anvil_fallback_test,
        ),
    ];
    run_test_suite(&tests)
}

fn run_as_merge_driver() -> ExitCode {
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
    ExitCode::SUCCESS
}

fn run_test_suite(tests: &[TestCase]) -> ExitCode {
    let mut failed = false;
    for (name, test_fn) in tests {
        match test_fn() {
            Ok(()) => println!("test {name} ... ok"),
            Err(err) => {
                eprintln!("test {name} FAILED: {err:#}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
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

fn init_repo_identity(repo: &Path) -> anyhow::Result<()> {
    git(repo, &["init", "-b", "main"])?;
    git(repo, &["config", "user.name", "coordinator"])?;
    git(repo, &["config", "user.email", "coord@example.com"])?;
    Ok(())
}

fn setup_repo_with_driver(repo: &Path) -> anyhow::Result<()> {
    init_repo_identity(repo)?;
    let exe = env::current_exe()?.to_string_lossy().replace('\\', "/");
    let repo_str = repo.to_string_lossy().replace('\\', "/");
    let driver_cmd =
        format!("ANVIL_AS_MERGE_DRIVER=1 ANVIL_MERGE_REPO=\"{repo_str}\" \"{exe}\" %O %A %B");
    install_git_merge_driver_with_cmd(repo, &driver_cmd)
}

fn run_missing_anvil_fallback_test() -> anyhow::Result<()> {
    let tmp = TempDir::new()?;
    let repo = tmp.path();
    init_repo_identity(repo)?;
    install_git_merge_driver(repo)?;

    let configured = git(repo, &["config", "--get", "merge.anvil-tasks.driver"])?;
    anyhow::ensure!(
        configured.trim() == DEFAULT_DRIVER_CMD,
        "unexpected driver config: {}",
        configured.trim()
    );

    let base_tasks = render_two_task_json("Alpha initial", "Beta initial");
    fs::write(repo.join("tasks.json"), &base_tasks)?;
    git(repo, &["add", "-A"])?;
    git(repo, &["commit", "-m", "chore: initial tasks.json"])?;

    git(repo, &["checkout", "-b", "dev-a"])?;
    fs::write(
        repo.join("tasks.json"),
        render_two_task_json("Alpha updated", "Beta initial"),
    )?;
    git(repo, &["commit", "-am", "feat(dev-a): update first task"])?;

    git(repo, &["checkout", "main"])?;
    git(repo, &["checkout", "-b", "dev-b"])?;
    fs::write(
        repo.join("tasks.json"),
        render_two_task_json("Alpha initial", "Beta updated"),
    )?;
    git(repo, &["commit", "-am", "feat(dev-b): update second task"])?;

    let sanitized_path = build_sanitized_path_without_anvil()?;
    git_with_path(repo, &["merge", "--no-edit", "dev-a"], &sanitized_path)?;

    let unmerged = git(repo, &["diff", "--name-only", "--diff-filter=U"])?;
    anyhow::ensure!(unmerged.trim().is_empty(), "unexpected conflict: {unmerged}");

    let merged = fs::read_to_string(repo.join("tasks.json"))?;
    anyhow::ensure!(!merged.contains("<<<<<<<"), "unexpected conflict markers");
    anyhow::ensure!(merged.contains("Alpha updated"), "missing dev-a edit");
    anyhow::ensure!(merged.contains("Beta updated"), "missing dev-b edit");
    Ok(())
}

fn render_two_task_json(first_request: &str, second_request: &str) -> String {
    format!(
        "{{\n  \"version\": 1,\n  \"task_count\": 2,\n  \"updated_at\": null,\n  \"tasks\": [\n    {{\n      \"task_id\": \"T-1\",\n      \"request\": \"{first_request}\",\n      \"status\": \"in_progress\",\n      \"created_by\": \"lead\",\n      \"contributors\": [\"lead\"],\n      \"branches\": [\"main\"],\n      \"files_touched\": [],\n      \"summary\": \"task one\",\n      \"entry_count\": 0\n    }},\n    {{\n      \"task_id\": \"T-2\",\n      \"request\": \"{second_request}\",\n      \"status\": \"in_progress\",\n      \"created_by\": \"lead\",\n      \"contributors\": [\"lead\"],\n      \"branches\": [\"main\"],\n      \"files_touched\": [],\n      \"summary\": \"task two\",\n      \"entry_count\": 0\n    }}\n  ]\n}}\n"
    )
}

fn build_sanitized_path_without_anvil() -> anyhow::Result<OsString> {
    let git_bin = if cfg!(windows) { "git.exe" } else { "git" };
    let git_exe = find_in_path(git_bin)
        .ok_or_else(|| anyhow::anyhow!("{git_bin} executable not found in PATH"))?;

    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(git_dir) = git_exe.parent() {
        push_dir_if_safe(&mut dirs, git_dir);
        if let Some(root) = git_dir.parent() {
            push_dir_if_safe(&mut dirs, &root.join("usr").join("bin"));
            push_dir_if_safe(&mut dirs, &root.join("bin"));
        }
    }

    let sh_bin = if cfg!(windows) { "sh.exe" } else { "sh" };
    if let Some(sh_dir) = find_in_path(sh_bin).as_deref().and_then(Path::parent) {
        push_dir_if_safe(&mut dirs, sh_dir);
    }

    #[cfg(unix)]
    for fallback in ["/bin", "/usr/bin"] {
        push_dir_if_safe(&mut dirs, Path::new(fallback));
    }

    anyhow::ensure!(!dirs.is_empty(), "failed to construct sanitized PATH without anvil");
    Ok(env::join_paths(dirs)?)
}

fn find_in_path(bin: &str) -> Option<PathBuf> {
    let path_var = env::var_os("PATH")?;
    env::split_paths(&path_var)
        .map(|dir| dir.join(bin))
        .find(|candidate| candidate.is_file())
}

fn push_dir_if_safe(dirs: &mut Vec<PathBuf>, dir: &Path) {
    if !dir.is_dir() || dir.join("anvil").is_file() || dir.join("anvil.exe").is_file() {
        return;
    }
    if !dirs.iter().any(|existing| existing == dir) {
        dirs.push(dir.to_path_buf());
    }
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
    anyhow::ensure!(!raw_tasks.contains("\"entries\""), "slim tasks.json must omit entries");
    anyhow::ensure!(
        !raw_tasks.contains("\"files_touched\""),
        "slim tasks.json must omit files_touched"
    );
    let index: TasksIndexFile = serde_json::from_str(&raw_tasks)?;
    anyhow::ensure!(index.version == 2, "expected v2 slim tasks.json");
    anyhow::ensure!(index.task_count == 1);

    let row = &index.tasks[0];
    anyhow::ensure!(row.entry_count == 3, "expected lead + dev-a + dev-b entry count");
    anyhow::ensure!(row.files_count == 2, "expected 2 touched files in count");
    anyhow::ensure!(row.contributors.iter().any(|c| c == "dev-a"));
    anyhow::ensure!(row.contributors.iter().any(|c| c == "dev-b"));
    anyhow::ensure!(row.progress_file == format!(".harness/tasks/{task_id}/progress.md"));

    let full = read_single_task(repo, task_id)?;
    anyhow::ensure!(full.entries.len() == 3, "expected lead + dev-a + dev-b entries on disk");
    anyhow::ensure!(full.entries.iter().any(|e| e.author == "dev-a"));
    anyhow::ensure!(full.entries.iter().any(|e| e.author == "dev-b"));
    anyhow::ensure!(full.files_touched.contains(&"src/storage.rs".to_owned()));
    anyhow::ensure!(full.files_touched.contains(&"src/api.rs".to_owned()));
    Ok(())
}

fn git(repo: &Path, args: &[&str]) -> anyhow::Result<String> {
    git_run(repo, args, None)
}

fn git_with_path(repo: &Path, args: &[&str], path: &OsStr) -> anyhow::Result<String> {
    git_run(repo, args, Some(path))
}

fn git_run(repo: &Path, args: &[&str], path_override: Option<&OsStr>) -> anyhow::Result<String> {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(repo);
    if let Some(path) = path_override {
        cmd.env("PATH", path);
    }
    let out = cmd.output()?;
    anyhow::ensure!(
        out.status.success(),
        "git {} failed:\nstdout: {}\nstderr: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
