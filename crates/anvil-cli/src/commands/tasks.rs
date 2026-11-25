//! `anvil tasks`: conflict-free team task ledger, JSON feature-list import/export, and verification.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use anvil_core::compressor::compress_output;
use anvil_core::config::AnvilConfig;
use anvil_core::proc_util::{TimedOutput, run_command_with_timeout};
use anvil_tasks::{
    ImportedFeatureList, ProgressUpdate, TaskStatus, TaskTarget, activate_task_from_meta,
    begin_existing_task, build_resume_brief, build_root_tasks_json, collect_task_files,
    consolidate_repo_tasks, export_feature_list, import_feature_list_in_group, list_task_groups,
    migrate_legacy_task_ids, parse_feature_list_json, read_single_task,
    record_active_command_outcome, record_curated_task_in_group, record_task_progress,
    record_task_skip, resolve_author, resolve_merge, sanitize_group_name, start_task_in_group,
    task_progress_rel_path, validate_curated_task_title, write_task_plan,
};

use crate::cli::subcmds::TasksSubcmd;
use crate::commands::ExitCodeError;
use crate::ui;
use crate::ui::file_picker;
use crate::ui::prompt::{self, PromptOutcome, SelectOption};

/// Executes `anvil tasks <subcommand>`.
pub fn execute(root: &Path, sub: TasksSubcmd) -> Result<()> {
    match sub {
        TasksSubcmd::Start { request, group, id, author } => cmd_start(
            root,
            &request,
            group.as_deref(),
            id.as_deref(),
            author.as_deref(),
        ),
        TasksSubcmd::Record { title, summary, status, group, id, files, author } => cmd_record(
            root,
            &title,
            &summary,
            parse_status(&status)?,
            TaskTarget { author: author.as_deref(), group: group.as_deref(), task_id: id.as_deref() },
            &files,
        ),
        TasksSubcmd::Skip { reason } => cmd_skip(root, &reason.join(" ")),
        TasksSubcmd::Update { .. } | TasksSubcmd::Done { .. } => execute_progress_cmd(root, sub),
        TasksSubcmd::Resume { task_id } => cmd_resume(root, task_id.as_deref()),
        TasksSubcmd::Plan { task_id, markdown_or_file, author } => {
            cmd_plan(root, &task_id, &markdown_or_file, author)
        }
        TasksSubcmd::Import { file, group, flat, tui, force, author } => {
            cmd_import(root, file, group.as_deref(), flat, tui, force, author.as_deref())
        }
        TasksSubcmd::Export { output, group, project } => {
            cmd_export(root, output.as_deref(), group.as_deref().or(project.as_deref()))
        }
        TasksSubcmd::Verify { task_id, record } => cmd_verify(root, &task_id, record),
        TasksSubcmd::MigrateIds => cmd_migrate_ids(root),
        TasksSubcmd::Build => {
            let consolidated = build_root_tasks_json(root)?;
            println!("built tasks.json: {} task(s)", consolidated.task_count);
            Ok(())
        }
        TasksSubcmd::List { group, status, limit } => {
            cmd_list(root, group.as_deref(), status.as_deref(), limit)
        }
        TasksSubcmd::MergeDriver { base, current, other } => {
            let consolidated = resolve_merge(&base, &current, &other, root)?;
            println!("merged tasks.json cleanly: {} task(s)", consolidated.task_count);
            Ok(())
        }
    }
}

fn execute_progress_cmd(root: &Path, sub: TasksSubcmd) -> Result<()> {
    match sub {
        TasksSubcmd::Update {
            task_id, summary, status, done, next, blocker, evidence, files, author,
        } => cmd_append(
            root,
            &task_id,
            ProgressUpdate {
                status: parse_status(&status)?,
                summary: &summary,
                done_steps: &done,
                next_steps: &next,
                blocker: blocker.as_deref(),
                evidence: evidence.as_deref(),
                files: &files,
            },
            author.as_deref(),
        ),
        TasksSubcmd::Done { task_id, summary, evidence, files, author } => cmd_append(
            root,
            &task_id,
            ProgressUpdate {
                status: TaskStatus::Done,
                summary: &summary,
                evidence: evidence.as_deref(),
                files: &files,
                ..ProgressUpdate::default()
            },
            author.as_deref(),
        ),
        _ => Ok(()),
    }
}

fn cmd_start(
    root: &Path,
    request: &[String],
    group: Option<&str>,
    id: Option<&str>,
    author: Option<&str>,
) -> Result<()> {
    let resolved_author = match author {
        Some(a) => a.to_owned(),
        None => resolve_author(root)?,
    };
    if request.len() == 1
        && group.is_none()
        && id.is_none()
        && read_single_task(root, &request[0]).is_ok()
    {
        let (task, _) = begin_existing_task(root, &request[0], &resolved_author, None)?;
        println!(
            "started existing task {} [{}] \"{}\" ({})",
            task.task_id,
            task.status,
            task.request,
            task_progress_rel_path(&task.task_id)
        );
        return Ok(());
    }
    let clean_title = validate_curated_task_title(&request.join(" "))?;
    let meta = start_task_in_group(root, &resolved_author, &clean_title, group, id)?;
    let consolidated = build_root_tasks_json(root)?;
    activate_task_from_meta(root, &meta)?;
    println!(
        "started task {} by {} ({}, tasks.json total={})",
        meta.task_id,
        meta.created_by,
        task_progress_rel_path(&meta.task_id),
        consolidated.task_count
    );
    Ok(())
}

fn cmd_record(
    root: &Path,
    title: &str,
    summary: &str,
    status: TaskStatus,
    target: TaskTarget<'_>,
    files: &[String],
) -> Result<()> {
    let (meta, entry) = record_curated_task_in_group(root, title, summary, status, files, target)?;
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
    explicit_group: Option<&str>,
    flat: bool,
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
    let target_group = resolve_import_group(&target_repo, &src_path, &list, explicit_group, flat)?;
    let report = import_feature_list_in_group(
        &target_repo,
        &list,
        target_group.as_deref(),
        author,
        force,
    )?;

    let dest_label = match target_group.as_deref() {
        Some(g) => format!("{}/.harness/tasks/{g}/", target_repo.display()),
        None => format!("{}/.harness/tasks/", target_repo.display()),
    };
    println!("{} {} {}", ui::green("✓"), ui::green("imported tasks into"), ui::bold(&dest_label));
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

fn resolve_import_group(
    target_repo: &Path,
    src_path: &Path,
    list: &ImportedFeatureList,
    explicit_group: Option<&str>,
    flat: bool,
) -> Result<Option<String>> {
    if flat {
        return Ok(None);
    }
    if let Some(g) = explicit_group {
        return sanitize_group_name(g).map(Some);
    }
    if !prompt::is_interactive_tui() {
        return Ok(None);
    }

    let suggested = suggest_group_name(src_path, list);
    let existing_groups = list_task_groups(target_repo);
    let default_idx = usize::from(list.project.is_none() && existing_groups.is_empty());
    let opts = [
        SelectOption::new(
            "Subdirectory .harness/tasks/<name>/",
            format!("grouped by module (default: {suggested})"),
        ),
        SelectOption::new(
            "Directly in .harness/tasks/",
            "flat root folder (no subdirectory)",
        ),
    ];
    let title = format!("Where should these {} tasks be stored?", list.features.len());
    match prompt::select(
        &title,
        "Task location",
        Some("Use a subdirectory if multiple modules share IDs like F001"),
        &opts,
        default_idx,
    )? {
        PromptOutcome::Cancelled => Err(ExitCodeError::silent(130).into()),
        PromptOutcome::Confirmed(1) => Ok(None),
        PromptOutcome::Confirmed(_) => {
            match prompt::text_input("Subdirectory name under .harness/tasks/", "Task group", &suggested)? {
                PromptOutcome::Confirmed(name) => sanitize_group_name(&name).map(Some),
                PromptOutcome::Cancelled => Err(ExitCodeError::silent(130).into()),
            }
        }
    }
}

fn suggest_group_name(src_path: &Path, list: &ImportedFeatureList) -> String {
    if let Some(proj) = list.project.as_deref()
        && let Ok(clean) = sanitize_group_name(proj)
    {
        return clean;
    }
    if let Some(parent) = src_path.parent().and_then(|p| p.file_name()).and_then(|s| s.to_str())
        && !matches!(parent.to_ascii_lowercase().as_str(), "tasks" | ".harness" | "harness")
        && let Ok(clean) = sanitize_group_name(parent)
    {
        return clean;
    }
    "features".to_owned()
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
        "workspace root `{}` has multiple subrepos; pass `--root <subrepo>`",
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
    let timeout = config.tier_budgets.tier1_timeout();
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
    let _ = record_active_command_outcome(root, verify_cmd, compressed.passed);
    if record {
        let author = resolve_author(root)?;
        let st = if compressed.passed {
            TaskStatus::Done
        } else {
            TaskStatus::InProgress
        };
        let ev = format!("verify (exit={}): {}", timed.status_code, compressed.summary);
        let _ = record_task_progress(
            root,
            task_id,
            &author,
            ProgressUpdate {
                status: st,
                summary: &ev,
                evidence: Some(&ev),
                ..ProgressUpdate::default()
            },
        )?;
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
    update: ProgressUpdate<'_>,
    author: Option<&str>,
) -> Result<()> {
    let resolved_author = match author {
        Some(a) => a.to_owned(),
        None => resolve_author(root)?,
    };
    let resolved_files = collect_task_files(root, Some(task_id), update.files);
    let scoped = ProgressUpdate {
        files: &resolved_files,
        ..update
    };
    let (task, entry) = record_task_progress(root, task_id, &resolved_author, scoped)?;
    println!(
        "updated {} [{}] entry={} files={} progress={}",
        task.task_id,
        entry.status,
        entry.entry_id,
        task.files_touched.len(),
        task_progress_rel_path(&task.task_id),
    );
    if entry.status == TaskStatus::Done
        && !task.files_touched.is_empty()
        && looks_like_learning(update.summary)
    {
        println!(
            "memory-hint: consider saving durable root-cause/decision via `anvil mem save` (or MCP `mem_save`) with related_files."
        );
    }
    Ok(())
}

fn cmd_resume(root: &Path, task_id: Option<&str>) -> Result<()> {
    let Some(brief) = build_resume_brief(root, task_id)? else {
        println!("no active or in_progress task to resume");
        return Ok(());
    };
    println!("{} {} [{}] {}", ui::accent("resume:"), ui::cyan(&brief.task_id), brief.status, brief.title);
    ui::print_kv("progress_file", &brief.progress_file);
    ui::print_kv("summary", &brief.summary);
    if !brief.done_steps.is_empty() {
        ui::print_kv("done_steps", &brief.done_steps.join("; "));
    }
    if !brief.next_steps.is_empty() {
        ui::print_kv("next_steps", &brief.next_steps.join("; "));
    }
    if let Some(blk) = brief.blocker.as_deref() {
        ui::print_kv("blocker", blk);
    }
    if let Some(verify) = brief.verify.as_deref() {
        ui::print_kv("verify", verify);
    }
    if !brief.touched_files.is_empty() {
        ui::print_kv("touched_files", &brief.touched_files.join(", "));
    }
    if let Some(cmd) = brief.last_command.as_deref() {
        let tag = match brief.last_command_passed {
            Some(true) => "PASS",
            Some(false) => "FAIL",
            None => "UNKNOWN",
        };
        ui::print_kv("last_command", &format!("[{tag}] {cmd}"));
    }
    if !brief.dirty_files.is_empty() {
        ui::print_kv("dirty_files", &brief.dirty_files.join(", "));
    }
    Ok(())
}

fn looks_like_learning(summary: &str) -> bool {
    let lower = summary.to_ascii_lowercase();
    ["fix", "bug", "error", "gotcha", "workaround", "auth", "oauth", "2fa", "corr", "fallback"]
        .iter()
        .any(|kw| lower.contains(kw))
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

fn cmd_list(
    root: &Path,
    group_filter: Option<&str>,
    status_raw: Option<&str>,
    limit: usize,
) -> Result<()> {
    let filter = status_raw.map(parse_status).transpose()?;
    let g_norm = group_filter.map(str::trim).filter(|s| !s.is_empty());
    let consolidated = consolidate_repo_tasks(root)?;
    let items: Vec<_> = consolidated
        .tasks
        .into_iter()
        .filter(|t| filter.is_none_or(|s| t.status == s))
        .filter(|t| {
            g_norm.is_none_or(|g| {
                t.task_id
                    .split_once('/')
                    .is_some_and(|(pfx, _)| pfx.eq_ignore_ascii_case(g))
                    || t.project.as_deref().is_some_and(|p| p.eq_ignore_ascii_case(g))
            })
        })
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
