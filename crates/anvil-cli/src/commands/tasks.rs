//! `anvil tasks`: conflict-free team task ledger, JSON feature-list import/export, and verification.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use anvil_core::compressor::compress_output;
use anvil_core::config::AnvilConfig;
use anvil_core::git::{self, DiffScope};
use anvil_core::proc_util::{TimedOutput, run_command_with_timeout};
use anvil_tasks::{
    TaskStatus, activate_task_from_meta, append_entry, append_entry_as,
    append_entry_with_evidence, build_root_tasks_json, clear_untracked_edits,
    consolidate_repo_tasks, export_feature_list, import_feature_list, load_active_task,
    load_untracked_edits, mark_summary_recorded_with_files, migrate_legacy_task_ids,
    parse_feature_list_json, read_single_task, record_curated_task_with_id, record_task_skip,
    resolve_author, resolve_merge, start_task_with_id, validate_curated_task_title,
    write_task_plan,
};

use crate::cli::subcmds::TasksSubcmd;
use crate::commands::ExitCodeError;
use crate::ui;
use crate::ui::file_picker;
use crate::ui::prompt::{self, PromptOutcome, SelectOption};

/// Executes `anvil tasks <subcommand>`.
pub fn execute(root: &Path, sub: TasksSubcmd) -> Result<()> {
    match sub {
        TasksSubcmd::Start { request, id, author } => {
            cmd_start(root, &request.join(" "), id.as_deref(), author.as_deref())
        }
        TasksSubcmd::Record { title, summary, status, id, files, author } => {
            cmd_record(root, &title, &summary, parse_status(&status)?, id.as_deref(), &files, author.as_deref())
        }
        TasksSubcmd::Skip { reason } => cmd_skip(root, &reason.join(" ")),
        TasksSubcmd::Update { task_id, summary, status, files, author } => {
            cmd_append(root, &task_id, &summary, parse_status(&status)?, &files, author.as_deref())
        }
        TasksSubcmd::Done { task_id, summary, files, author } => {
            cmd_append(root, &task_id, &summary, TaskStatus::Done, &files, author.as_deref())
        }
        TasksSubcmd::Plan { task_id, markdown_or_file, author } => {
            cmd_plan(root, &task_id, &markdown_or_file, author)
        }
        TasksSubcmd::Import { file, tui, force, author } => {
            cmd_import(root, file, tui, force, author.as_deref())
        }
        TasksSubcmd::Export { output, project } => {
            cmd_export(root, output.as_deref(), project.as_deref())
        }
        TasksSubcmd::Verify { task_id, record } => cmd_verify(root, &task_id, record),
        TasksSubcmd::MigrateIds => cmd_migrate_ids(root),
        TasksSubcmd::Build => {
            let consolidated = build_root_tasks_json(root)?;
            println!("built tasks.json: {} task(s)", consolidated.task_count);
            Ok(())
        }
        TasksSubcmd::List { status, limit } => cmd_list(root, status.as_deref(), limit),
        TasksSubcmd::MergeDriver { base, current, other } => {
            let consolidated = resolve_merge(&base, &current, &other, root)?;
            println!("merged tasks.json cleanly: {} task(s)", consolidated.task_count);
            Ok(())
        }
    }
}

fn cmd_start(root: &Path, request: &str, id: Option<&str>, author: Option<&str>) -> Result<()> {
    let clean_title = validate_curated_task_title(request)?;
    let resolved_author = match author {
        Some(a) => a.to_owned(),
        None => resolve_author(root)?,
    };
    let meta = start_task_with_id(root, &resolved_author, &clean_title, id)?;
    let consolidated = build_root_tasks_json(root)?;
    activate_task_from_meta(root, &meta)?;
    println!(
        "started task {} by {} (tasks.json total={})",
        meta.task_id, meta.created_by, consolidated.task_count
    );
    Ok(())
}

fn cmd_record(
    root: &Path,
    title: &str,
    summary: &str,
    status: TaskStatus,
    id: Option<&str>,
    files: &[String],
    author: Option<&str>,
) -> Result<()> {
    let (meta, entry) = record_curated_task_with_id(root, title, summary, status, files, author, id)?;
    println!(
        "recorded task {} [{}] \"{}\" files={}",
        meta.task_id,
        entry.status,
        meta.request,
        entry.files_touched.len()
    );
    Ok(())
}

fn cmd_migrate_ids(root: &Path) -> Result<()> {
    let migrated = migrate_legacy_task_ids(root)?;
    if migrated.is_empty() {
        println!("no legacy task IDs found in .harness/tasks/");
        return Ok(());
    }
    println!("{} migrated {} legacy task ID(s):", ui::green("✓"), migrated.len());
    for (old_id, new_id) in migrated {
        println!("  {} -> {}", ui::dim(&old_id), ui::cyan(&new_id));
    }
    Ok(())
}

fn cmd_skip(root: &Path, reason: &str) -> Result<()> {
    let state = record_task_skip(root, reason)?;
    println!(
        "skipped task creation: {}",
        state.skip_reason.unwrap_or_default()
    );
    Ok(())
}

fn cmd_import(
    root: &Path,
    file: Option<PathBuf>,
    force_tui: bool,
    force: bool,
    author: Option<&str>,
) -> Result<()> {
    let src_path = match file {
        Some(path) => path,
        None => match file_picker::pick_json_file(root, force_tui)? {
            PromptOutcome::Confirmed(path) => path,
            PromptOutcome::Cancelled => return Err(ExitCodeError::silent(130).into()),
        },
    };
    let raw = fs::read_to_string(&src_path)
        .with_context(|| format!("failed to read {}", src_path.display()))?;
    let list = parse_feature_list_json(&raw)?;
    let target_repo = resolve_import_target_repo(root, list.project.as_deref())?;
    let report = import_feature_list(&target_repo, &list, author, force)?;

    println!(
        "{} {} {}",
        ui::green("✓"),
        ui::green("imported tasks into"),
        ui::bold(&target_repo.display().to_string())
    );
    if let Some(proj) = report.project.as_deref() {
        ui::print_kv("project", &ui::cyan(proj));
    }
    ui::print_kv(
        "changes",
        &format!(
            "imported={}  updated={}  skipped={}  total={}",
            report.imported, report.updated, report.skipped, report.total_tasks
        ),
    );
    Ok(())
}

fn resolve_import_target_repo(root: &Path, project: Option<&str>) -> Result<PathBuf> {
    if root.join(".harness").is_dir() {
        return Ok(root.to_path_buf());
    }
    let subrepos = discover_initialized_subrepos(root);
    if subrepos.is_empty() {
        return Ok(root.to_path_buf());
    }
    let default_idx = find_project_match_idx(&subrepos, project).unwrap_or(0);
    if prompt::is_interactive_tui() {
        let opts: Vec<SelectOption> = subrepos
            .iter()
            .map(|(name, dir)| SelectOption::new(name.as_str(), dir.display().to_string()))
            .collect();
        let subtitle = project
            .map(|p| format!("Feature list project `{p}` · umbrella root has multiple repos"))
            .unwrap_or_else(|| "Umbrella workspace root has multiple initialized repos".to_owned());
        match prompt::select(
            "Import into which repository?",
            "Target repository",
            Some(&subtitle),
            &opts,
            default_idx,
        )? {
            PromptOutcome::Confirmed(idx) => return Ok(subrepos[idx].1.clone()),
            PromptOutcome::Cancelled => return Err(ExitCodeError::silent(130).into()),
        }
    }
    if let Some(idx) = find_project_match_idx(&subrepos, project) {
        return Ok(subrepos[idx].1.clone());
    }
    bail!(
        "workspace root `{}` has multiple subrepos; pass `--repo <subrepo>`",
        root.display()
    );
}

fn discover_initialized_subrepos(root: &Path) -> Vec<(String, PathBuf)> {
    let Ok(rd) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() && p.join(".harness/config.json").is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            out.push((name, p));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn find_project_match_idx(subrepos: &[(String, PathBuf)], project: Option<&str>) -> Option<usize> {
    let proj = project?.trim().to_ascii_lowercase();
    if proj.is_empty() {
        return None;
    }
    subrepos
        .iter()
        .position(|(name, _)| name.eq_ignore_ascii_case(&proj))
        .or_else(|| {
            subrepos
                .iter()
                .position(|(name, _)| name.to_ascii_lowercase().ends_with(&format!("-{proj}")))
        })
        .or_else(|| {
            subrepos
                .iter()
                .position(|(name, _)| name.to_ascii_lowercase().contains(&proj))
        })
}

fn cmd_export(root: &Path, output: Option<&Path>, project: Option<&str>) -> Result<()> {
    let payload = export_feature_list(root, project)?;
    let mut json = serde_json::to_string_pretty(&payload)?;
    json.push('\n');
    if let Some(path) = output {
        fs::write(path, json.as_bytes())
            .with_context(|| format!("failed to write {}", path.display()))?;
        println!("{} exported {}", ui::green("✓"), ui::cyan(&path.display().to_string()));
    } else {
        print!("{json}");
    }
    Ok(())
}

fn cmd_verify(root: &Path, task_id: &str, record: bool) -> Result<()> {
    let task = read_single_task(root, task_id)?;
    let Some(verify_cmd) = task.verify.as_deref() else {
        println!(
            "{} {} (`verify: null`) — manual/runtime acceptance required",
            ui::yellow("MANUAL"),
            ui::cyan(task_id)
        );
        return Ok(());
    };
    let config = AnvilConfig::load_or_default(root)?;
    let timeout = Duration::from_secs(config.tier_budgets.tier1_secs.max(60));
    let timed = run_verify_shell(root, verify_cmd, timeout)?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&timed.stdout),
        String::from_utf8_lossy(&timed.stderr)
    );
    let compressed = compress_output(
        Some(root),
        &format!("verify-{task_id}"),
        timed.status_code,
        &combined,
        config.tier_budgets.compressor_max_lines,
        config.tier_budgets.compressor_max_tokens,
    )?;
    if record {
        let author = resolve_author(root)?;
        let st = if compressed.passed {
            TaskStatus::Done
        } else {
            TaskStatus::InProgress
        };
        let ev = format!("verify (exit={}): {}", timed.status_code, compressed.summary);
        let _ = append_entry_with_evidence(root, task_id, &author, st, &ev, Some(&ev), &[])?;
        let _ = build_root_tasks_json(root)?;
    }
    print_verify_result(task_id, &compressed);
    if !compressed.passed {
        return Err(ExitCodeError::silent(timed.status_code).into());
    }
    Ok(())
}

fn print_verify_result(task_id: &str, compressed: &anvil_core::traits::CompressedOutput) {
    if compressed.passed {
        println!("{} {}: {}", ui::green("PASS"), ui::cyan(task_id), compressed.summary);
    } else {
        println!("{} {}: {}", ui::red("FAIL"), ui::cyan(task_id), compressed.summary);
        for line in &compressed.lines {
            println!("  {line}");
        }
    }
}

fn run_verify_shell(root: &Path, raw_cmd: &str, timeout: Duration) -> Result<TimedOutput> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut command = Command::new("cmd");
        command
            .args(["/D", "/S", "/C"])
            .raw_arg(format!("\"{raw_cmd}\""))
            .current_dir(root);
        run_command_with_timeout(&mut command, timeout)
            .with_context(|| format!("failed to run verify command `{raw_cmd}`"))
    }
    #[cfg(not(windows))]
    {
        let mut command = Command::new("sh");
        command.args(["-c", raw_cmd]).current_dir(root);
        run_command_with_timeout(&mut command, timeout)
            .with_context(|| format!("failed to run verify command `{raw_cmd}`"))
    }
}

fn cmd_append(
    root: &Path,
    task_id: &str,
    summary: &str,
    status: TaskStatus,
    files: &[String],
    author: Option<&str>,
) -> Result<()> {
    let resolved_files = resolve_touched_files(root, task_id, files);
    let entry = match author {
        Some(a) => append_entry_as(root, task_id, a, status, summary, &resolved_files)?,
        None => append_entry(root, task_id, status, summary, &resolved_files)?,
    };
    let consolidated = build_root_tasks_json(root)?;
    mark_summary_recorded_with_files(root, task_id, status, &entry.files_touched)?;
    clear_untracked_edits(root);
    println!(
        "updated {} [{}] entry={} files={} (tasks.json total={})",
        task_id, entry.status, entry.entry_id, entry.files_touched.len(), consolidated.task_count
    );
    if status == TaskStatus::Done && !entry.files_touched.is_empty() && looks_like_learning(summary)
    {
        println!(
            "memory-hint: consider saving durable root-cause/decision via `anvil mem save` (or MCP `mem_save`) with related_files."
        );
    }
    Ok(())
}

fn looks_like_learning(summary: &str) -> bool {
    let lower = summary.to_ascii_lowercase();
    ["fix", "bug", "error", "gotcha", "workaround", "auth", "oauth", "2fa", "corr", "fallback"]
        .iter()
        .any(|kw| lower.contains(kw))
}

fn resolve_touched_files(root: &Path, task_id: &str, explicit: &[String]) -> Vec<String> {
    if !explicit.is_empty() {
        return explicit.to_vec();
    }
    let mut set: BTreeSet<String> = load_untracked_edits(root).into_iter().collect();
    if let Ok(Some(active)) = load_active_task(root)
        && active.task_id == task_id
    {
        for f in active.touched_files {
            if is_project_file(&f) {
                set.insert(f);
            }
        }
    }
    if let Ok(changed) = git::changed_files(root, DiffScope::WorkingTree) {
        for f in changed {
            let norm = f.replace('\\', "/");
            if is_project_file(&norm) {
                set.insert(norm);
            }
        }
    }
    set.into_iter().collect()
}

fn is_project_file(path: &str) -> bool {
    let trimmed = path.trim();
    !trimmed.is_empty() && !trimmed.starts_with(".harness/") && trimmed != "tasks.json"
}

fn cmd_plan(root: &Path, task_id: &str, markdown_or_file: &str, author: Option<String>) -> Result<()> {
    let resolved_author = match author {
        Some(a) => a,
        None => resolve_author(root)?,
    };
    let md = if Path::new(markdown_or_file).is_file() {
        fs::read_to_string(markdown_or_file)?
    } else {
        markdown_or_file.to_owned()
    };
    let path = write_task_plan(root, task_id, &resolved_author, &md)?;
    println!("wrote plan: {}", path.display());
    Ok(())
}

fn cmd_list(root: &Path, status_raw: Option<&str>, limit: usize) -> Result<()> {
    let filter = status_raw.map(parse_status).transpose()?;
    let consolidated = consolidate_repo_tasks(root)?;
    let items: Vec<_> = consolidated
        .tasks
        .into_iter()
        .filter(|t| filter.is_none_or(|s| t.status == s))
        .take(limit.max(1))
        .collect();
    if items.is_empty() {
        println!("no tasks recorded in .harness/tasks/");
        return Ok(());
    }
    for task in items {
        let verify_tag = if task.verify.is_some() { " [verify]" } else { "" };
        println!("{} [{}] {}{}", task.task_id, task.status, task.request, verify_tag);
    }
    Ok(())
}

fn parse_status(raw: &str) -> Result<TaskStatus> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "not_started" | "not-started" | "pending" | "todo" | "open" => Ok(TaskStatus::NotStarted),
        "in_progress" | "in-progress" | "progress" | "wip" => Ok(TaskStatus::InProgress),
        "blocked" => Ok(TaskStatus::Blocked),
        "done" | "completed" | "verified" => Ok(TaskStatus::Done),
        other => bail!("invalid task status `{other}` (expected not_started|in_progress|blocked|done)"),
    }
}
