//! Non-destructive repository initialization and thin-adapter synchronization.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use anvil_core::config::{AdapterTarget, AnvilConfig, StackKind};
use anvil_core::fs_util::atomic_write;
use anvil_core::stack_detector;

use crate::{antigravity, claude_code, codex, cursor, templates, windsurf};

const MANAGED_START: &str = "<!-- anvil:managed:start -->";
const MANAGED_END: &str = "<!-- anvil:managed:end -->";

/// Summary of actions taken by [`init_repo`] or [`sync_repo`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncReport {
    pub created_dirs: Vec<PathBuf>,
    pub written_files: Vec<PathBuf>,
    pub preserved_skills: Vec<String>,
}

/// Subproject metadata passed to [`init_umbrella_root`] for Thin Umbrella workspace indices.
#[derive(Debug, Clone)]
pub struct UmbrellaSubrepo {
    pub rel_path: String,
    pub abs_path: PathBuf,
    pub stacks: Vec<StackKind>,
}

/// Initializes a lightweight umbrella root (`AGENTS.md`/`CLAUDE.md`, IDE hook routers, and
/// per-subrepo MCP servers) WITHOUT creating duplicate `.harness/` or `tasks.json` in `root`.
pub fn init_umbrella_root(
    root: &Path,
    explicit_adapter: Option<AdapterTarget>,
    subrepos: &[UmbrellaSubrepo],
) -> Result<(AdapterTarget, SyncReport)> {
    let target = explicit_adapter.unwrap_or_else(|| infer_default_adapters(root));
    let mut report = SyncReport::default();
    let mcp_entries: Vec<(String, PathBuf)> = subrepos
        .iter()
        .map(|s| (sanitize_subrepo_slug(&s.rel_path), s.abs_path.clone()))
        .collect();

    if target.includes_antigravity() {
        record_write(
            antigravity::config::hooks_config_path(root),
            antigravity::config::merge_hooks_config(root)?,
            &mut report,
        );
        record_write(
            antigravity::config::mcp_config_path(root),
            antigravity::config::merge_umbrella_mcp_config(root, &mcp_entries)?,
            &mut report,
        );
    }
    if target.includes_claude() {
        record_write(
            claude_code::config::settings_config_path(root),
            claude_code::config::merge_settings_config(root)?,
            &mut report,
        );
        record_write(
            claude_code::config::mcp_config_path(root),
            claude_code::config::merge_umbrella_mcp_config(root, &mcp_entries)?,
            &mut report,
        );
        let doc = render_umbrella_doc("CLAUDE.md — Multi-Repo Workspace Router", subrepos);
        sync_managed_doc(&root.join("CLAUDE.md"), &doc, &mut report)?;
    }
    sync_umbrella_secondary_ides(root, target, subrepos, &mcp_entries, &mut report)?;
    if target.includes_antigravity() || should_sync_codex(root, target) {
        let doc = render_umbrella_doc("AGENTS.md — Multi-Repo Workspace Router", subrepos);
        sync_managed_doc(&root.join("AGENTS.md"), &doc, &mut report)?;
    }
    Ok((target, report))
}

fn sync_umbrella_secondary_ides(
    root: &Path,
    target: AdapterTarget,
    subrepos: &[UmbrellaSubrepo],
    mcp_entries: &[(String, PathBuf)],
    report: &mut SyncReport,
) -> Result<()> {
    if should_sync_cursor(root, target) {
        record_write(
            cursor::mcp_config_path(root),
            cursor::merge_umbrella_mcp_config(root, mcp_entries)?,
            report,
        );
        let doc = render_umbrella_doc("Cursor Rules — Multi-Repo Workspace Router", subrepos);
        record_write(
            cursor::rules_path(root),
            cursor::sync_rules_with_template(root, &doc)?,
            report,
        );
    }
    if should_sync_windsurf(root, target) {
        record_write(
            windsurf::mcp_config_path(root),
            windsurf::merge_umbrella_mcp_config(root, mcp_entries)?,
            report,
        );
        let doc = render_umbrella_doc("Windsurf Rules — Multi-Repo Workspace Router", subrepos);
        record_write(
            windsurf::rules_path(root),
            windsurf::sync_rules_with_template(root, &doc)?,
            report,
        );
    }
    if should_sync_codex(root, target) {
        record_write(
            codex::config_path(root),
            codex::merge_umbrella_config_toml(root, mcp_entries)?,
            report,
        );
    }
    Ok(())
}

/// Discovers initialized child repositories under a Thin Umbrella workspace root (depth 1-2).
pub fn discover_umbrella_subrepos(root: &Path) -> Vec<UmbrellaSubrepo> {
    let mut found = Vec::new();
    collect_umbrella_depth(root, root, 0, &mut found);
    found.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    found
}

fn collect_umbrella_depth(
    root: &Path,
    dir: &Path,
    depth: usize,
    out: &mut Vec<UmbrellaSubrepo>,
) {
    if depth >= 2 {
        return;
    }
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    for entry in rd.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if !ft.is_dir() || ft.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.')
            || matches!(
                name_str.as_ref(),
                "node_modules" | "target" | "dist" | "build" | "vendor"
            )
        {
            continue;
        }
        let sub_dir = entry.path();
        if sub_dir.join(".harness").join("config.json").is_file() {
            let rel = anvil_core::fs_util::relativize_path(root, &sub_dir);
            let stacks = AnvilConfig::load(&sub_dir).map(|c| c.stacks).unwrap_or_default();
            out.push(UmbrellaSubrepo {
                rel_path: rel,
                abs_path: sub_dir,
                stacks,
            });
            continue;
        }
        collect_umbrella_depth(root, &sub_dir, depth + 1, out);
    }
}

fn sanitize_subrepo_slug(rel_path: &str) -> String {
    let cleaned: String = rel_path
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() { "repo".to_owned() } else { trimmed.to_owned() }
}

fn render_umbrella_doc(heading: &str, subrepos: &[UmbrellaSubrepo]) -> String {
    let mut rows = String::new();
    for s in subrepos {
        let slug = sanitize_subrepo_slug(&s.rel_path);
        let stacks = if s.stacks.is_empty() {
            "generic".to_owned()
        } else {
            s.stacks.iter().map(|st| format!("{st:?}")).collect::<Vec<_>>().join(", ")
        };
        rows.push_str(&format!(
            "- **`{}`** ({stacks}) — MCP server `anvil-{slug}`, CLI: `anvil --root {} <cmd>`\n",
            s.rel_path, s.rel_path
        ));
    }
    format!(
        "{MANAGED_START}\n\n# {heading}\n\n> **Availability Guard**: Only apply this `anvil:managed` block if the `anvil` binary is on `PATH` (or an `anvil-*` MCP server is active). If `anvil` is not installed in this environment, **skip this entire block**—do NOT invoke `anvil` commands or treat `.harness/` as an active workflow/source of truth; use each repo's native tools instead.\n> Thin multi-repo workspace root. Each independent repository owns its own `.harness/` and `tasks.json` (no duplicate state at root).\n\n## Subrepositories\n\n{rows}\n## Working Across Subrepositories\n\n- Hooks route edits automatically by target file path into each repo's `.harness/`.\n- Run verification per repo (`anvil --root <subrepo> check`) or query any repo's graph/memory/tasks via its `anvil-<repo>` MCP server.\n\n{MANAGED_END}\n"
    )
}

/// Initializes `.harness/` in `repo_root` and synchronizes adapters (`auto` detection).
pub fn init_repo(repo_root: &Path) -> Result<SyncReport> {
    init_repo_with_adapter(repo_root, None)
}

/// Initializes `.harness/` in `repo_root` with an optional explicit [`AdapterTarget`].
pub fn init_repo_with_adapter(
    repo_root: &Path,
    explicit_adapter: Option<AdapterTarget>,
) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    ensure_harness_skeleton(repo_root, &mut report)?;

    let config = upsert_repo_config(repo_root, explicit_adapter, &mut report)?;
    sync_into(repo_root, config.adapters, &mut report)?;
    Ok(report)
}

/// Synchronizes adapter skills, hook/MCP configs, `.gitignore`, and instructions idempotently.
pub fn sync_repo(repo_root: &Path) -> Result<SyncReport> {
    sync_repo_with_adapter(repo_root, None)
}

/// Synchronizes adapters with an optional explicit [`AdapterTarget`] override, upgrading config tools.
pub fn sync_repo_with_adapter(
    repo_root: &Path,
    explicit_adapter: Option<AdapterTarget>,
) -> Result<SyncReport> {
    let mut report = SyncReport::default();
    ensure_harness_skeleton(repo_root, &mut report)?;

    let config = upsert_repo_config(repo_root, explicit_adapter, &mut report)?;
    sync_into(repo_root, config.adapters, &mut report)?;
    Ok(report)
}

fn upsert_repo_config(
    repo_root: &Path,
    explicit_adapter: Option<AdapterTarget>,
    report: &mut SyncReport,
) -> Result<AnvilConfig> {
    let config_path = AnvilConfig::path(repo_root);
    let detection = stack_detector::detect_workspace(repo_root)?;

    if !config_path.exists() {
        let mut config = AnvilConfig {
            adapters: explicit_adapter.unwrap_or_else(|| infer_default_adapters(repo_root)),
            ..AnvilConfig::default()
        };
        detection.apply_to_config(&mut config);
        config.save(repo_root)?;
        report.written_files.push(config_path);
        return Ok(config);
    }

    let mut config = AnvilConfig::load(repo_root)?;
    let mut changed = detection.merge_into_config(&mut config);
    if let Some(target) = explicit_adapter
        && config.adapters != target
    {
        config.adapters = target;
        changed = true;
    }
    if changed {
        config.save(repo_root)?;
        report.written_files.push(config_path);
    }
    Ok(config)
}

fn infer_default_adapters(repo_root: &Path) -> AdapterTarget {
    if let Ok(env_raw) = std::env::var("ANVIL_ADAPTER")
        && let Ok(Some(parsed)) = AdapterTarget::parse_cli(&env_raw)
    {
        return parsed;
    }
    let has_codex = repo_root.join(".codex").exists();
    let has_secondary =
        has_codex || repo_root.join(".cursor").exists() || repo_root.join(".windsurf").exists();
    let has_agy =
        repo_root.join(".agents").exists() || (repo_root.join("AGENTS.md").exists() && !has_secondary);
    let has_claude = repo_root.join(".claude").exists()
        || repo_root.join("CLAUDE.md").exists()
        || repo_root.join(".mcp.json").exists();
    match (has_agy, has_claude) {
        (true, false) => AdapterTarget::Antigravity,
        (false, true) => AdapterTarget::Claude,
        (true, true) => AdapterTarget::All,
        (false, false) => infer_secondary_ide_adapter(repo_root),
    }
}

fn infer_secondary_ide_adapter(repo_root: &Path) -> AdapterTarget {
    let has_cursor = repo_root.join(".cursor").exists();
    let has_windsurf = repo_root.join(".windsurf").exists();
    let has_codex = repo_root.join(".codex").exists();
    match (has_cursor, has_windsurf, has_codex) {
        (true, false, false) => AdapterTarget::Cursor,
        (false, true, false) => AdapterTarget::Windsurf,
        (false, false, true) => AdapterTarget::Codex,
        _ => AdapterTarget::All,
    }
}

fn should_sync_cursor(repo_root: &Path, target: AdapterTarget) -> bool {
    target.includes_cursor() || (target == AdapterTarget::All && repo_root.join(".cursor").exists())
}

fn should_sync_windsurf(repo_root: &Path, target: AdapterTarget) -> bool {
    target.includes_windsurf()
        || (target == AdapterTarget::All && repo_root.join(".windsurf").exists())
}

fn should_sync_codex(repo_root: &Path, target: AdapterTarget) -> bool {
    target.includes_codex() || (target == AdapterTarget::All && repo_root.join(".codex").exists())
}

fn sync_into(
    repo_root: &Path,
    target: AdapterTarget,
    report: &mut SyncReport,
) -> Result<()> {
    let sync_cursor = should_sync_cursor(repo_root, target);
    let sync_windsurf = should_sync_windsurf(repo_root, target);
    let sync_codex = should_sync_codex(repo_root, target);

    sync_skills_targets(repo_root, target, sync_cursor, report)?;

    if target.includes_antigravity() {
        record_write(antigravity::config::hooks_config_path(repo_root), antigravity::config::merge_hooks_config(repo_root)?, report);
        record_write(antigravity::config::mcp_config_path(repo_root), antigravity::config::merge_mcp_config(repo_root)?, report);
    }
    if target.includes_claude() {
        record_write(claude_code::config::settings_config_path(repo_root), claude_code::config::merge_settings_config(repo_root)?, report);
        record_write(claude_code::config::mcp_config_path(repo_root), claude_code::config::merge_mcp_config(repo_root)?, report);
        sync_managed_doc(&repo_root.join("CLAUDE.md"), templates::CLAUDE_MD, report)?;
    }
    if sync_cursor {
        record_write(cursor::mcp_config_path(repo_root), cursor::merge_mcp_config(repo_root)?, report);
        record_write(cursor::rules_path(repo_root), cursor::sync_rules(repo_root)?, report);
    }
    if sync_windsurf {
        record_write(windsurf::mcp_config_path(repo_root), windsurf::merge_mcp_config(repo_root)?, report);
        record_write(windsurf::rules_path(repo_root), windsurf::sync_rules(repo_root)?, report);
    }
    if sync_codex {
        record_write(codex::config_path(repo_root), codex::merge_config_toml(repo_root)?, report);
    }
    if target.includes_antigravity() {
        sync_managed_doc(&repo_root.join("AGENTS.md"), templates::AGENTS_MD, report)?;
    } else if sync_codex {
        sync_managed_doc(
            &repo_root.join("AGENTS.md"),
            templates::CODEX_AGENTS_MD,
            report,
        )?;
    }

    sync_gitignore(repo_root, report)?;
    Ok(())
}

fn record_write(path: PathBuf, changed: bool, report: &mut SyncReport) {
    if changed {
        report.written_files.push(path);
    }
}

pub(crate) fn write_if_changed(path: &Path, content: &str) -> Result<bool> {
    if fs::read_to_string(path).is_ok_and(|existing| existing == content) {
        return Ok(false);
    }
    atomic_write(path, content.as_bytes())?;
    Ok(true)
}

fn ensure_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("failed to create directory {}", dir.display()))
}

fn ensure_harness_skeleton(repo_root: &Path, report: &mut SyncReport) -> Result<()> {
    let harness = repo_root.join(".harness");
    for rel in ["rules", "candidates", "memory/chunks", "tasks", ".state"] {
        let dir = harness.join(rel);
        if !dir.exists() {
            ensure_dir(&dir)?;
            report.created_dirs.push(dir);
        }
    }
    Ok(())
}

fn sync_skills_targets(
    repo_root: &Path,
    target: AdapterTarget,
    sync_cursor: bool,
    report: &mut SyncReport,
) -> Result<()> {
    let custom_root = repo_root.join(".harness").join("skills");
    let custom_names = list_subdir_names(&custom_root)?;
    let builtin_names: BTreeSet<String> = templates::EMBEDDED_SKILLS
        .iter()
        .map(|s| s.name.to_owned())
        .collect();

    let mut targets = Vec::new();
    if target.includes_antigravity() {
        targets.push(repo_root.join(".agents").join("skills"));
    }
    if target.includes_claude() {
        targets.push(repo_root.join(".claude").join("skills"));
    }
    if sync_cursor {
        targets.push(repo_root.join(".cursor").join("skills"));
    }

    let mut preserved = BTreeSet::new();
    for target_root in &targets {
        ensure_dir(target_root)?;
        for existing in list_subdir_names(target_root)? {
            if !custom_names.contains(&existing) && !builtin_names.contains(&existing) {
                preserved.insert(existing);
            }
        }
        sync_target_skill_dir(&custom_root, &custom_names, target_root, report)?;
    }
    report.preserved_skills.extend(preserved);
    Ok(())
}

fn sync_target_skill_dir(
    custom_root: &Path,
    custom_names: &BTreeSet<String>,
    target_root: &Path,
    report: &mut SyncReport,
) -> Result<()> {
    for skill in templates::EMBEDDED_SKILLS {
        if custom_names.contains(skill.name) {
            continue;
        }
        let dst = target_root.join(skill.name).join("SKILL.md");
        if write_if_changed(&dst, skill.skill_md)? {
            report.written_files.push(dst);
        }
    }
    for skill_name in custom_names {
        let src_dir = custom_root.join(skill_name);
        let dst_dir = target_root.join(skill_name);
        copy_dir_non_destructively(&src_dir, &dst_dir, report)?;
    }
    Ok(())
}

fn list_subdir_names(dir: &Path) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    if !dir.exists() {
        return Ok(names);
    }
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && let Some(name) = entry.file_name().to_str()
        {
            names.insert(name.to_owned());
        }
    }
    Ok(names)
}

fn copy_dir_non_destructively(
    src: &Path,
    dst: &Path,
    report: &mut SyncReport,
) -> Result<()> {
    ensure_dir(dst)?;
    for entry in fs::read_dir(src).with_context(|| format!("failed to read {}", src.display()))? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_non_destructively(&src_path, &dst_path, report)?;
        } else {
            let bytes = fs::read(&src_path)
                .with_context(|| format!("failed to read {}", src_path.display()))?;
            let unchanged = fs::read(&dst_path).is_ok_and(|existing| existing == bytes);
            if !unchanged {
                atomic_write(&dst_path, &bytes)?;
                report.written_files.push(dst_path);
            }
        }
    }
    Ok(())
}

fn sync_gitignore(repo_root: &Path, report: &mut SyncReport) -> Result<()> {
    let path = repo_root.join(".gitignore");
    let existing = if path.exists() {
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?
    } else {
        String::new()
    };

    let has_line = |target: &str| {
        let trimmed = target.trim_end_matches('/');
        existing
            .lines()
            .map(str::trim)
            .any(|line| line == target || line == trimmed)
    };
    let need_state = !has_line(".harness/.state/");
    let need_worktrees = !has_line(".harness/.worktrees/");
    if !need_state && !need_worktrees {
        return Ok(());
    }

    let mut updated = existing;
    if !updated.is_empty() && !updated.ends_with('\n') {
        updated.push('\n');
    }
    if need_state {
        updated.push_str(".harness/.state/\n");
    }
    if need_worktrees {
        updated.push_str(".harness/.worktrees/\n");
    }
    if write_if_changed(&path, &updated)? {
        report.written_files.push(path);
    }
    Ok(())
}

pub(crate) fn read_optional_string(path: &Path) -> Result<Option<String>> {
    if !path.exists() {
        return Ok(None);
    }
    fs::read_to_string(path)
        .with_context(|| format!("failed to read {}", path.display()))
        .map(Some)
}

pub(crate) fn sync_managed_file(path: &Path, template: &str) -> Result<bool> {
    let existing = read_optional_string(path)?;
    let merged = merge_managed_markdown(existing.as_deref(), template);
    write_if_changed(path, &merged)
}

pub(crate) fn sync_managed_doc(path: &Path, template: &str, report: &mut SyncReport) -> Result<()> {
    if sync_managed_file(path, template)? {
        report.written_files.push(path.to_path_buf());
    }
    Ok(())
}

pub(crate) fn merge_managed_markdown(existing: Option<&str>, template: &str) -> String {
    let Some(current) = existing else {
        return template.to_owned();
    };
    if current.trim().is_empty() {
        return template.to_owned();
    }
    if let (Some(start), Some(end)) = (current.find(MANAGED_START), current.find(MANAGED_END))
        && start < end
    {
        let before = &current[..start];
        let after = &current[end + MANAGED_END.len()..];
        let replacement = template.trim_end();
        return format!("{before}{replacement}{after}");
    }
    let sep = if current.ends_with('\n') { "\n" } else { "\n\n" };
    format!("{current}{sep}{template}")
}
