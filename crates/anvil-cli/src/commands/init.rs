//! `anvil init`: single-repo and multi-subproject `.harness/`, adapter, git, graph, and task setup.

mod architecture;
mod prereqs;

use std::fs;
use std::io::{self, BufRead, IsTerminal, Write};
use std::path::Path;

use anvil_adapters::sync::{self, UmbrellaSubrepo};
use anvil_core::config::{AdapterTarget, AnvilConfig, SonarConfig};
use anvil_core::sonar::parse_sonar_properties;
use anvil_core::stack_detector::{self, DetectedRepo};
use anvil_graph::CodeGraphEngine;
use anvil_tasks::{build_root_tasks_json, install_git_merge_driver};
use anyhow::{Context, Result, bail};

use crate::commands::ExitCodeError;
use crate::ui;
use crate::ui::prompt::{self, PromptOutcome, SelectOption};

/// CLI parameters passed to `anvil init`.
#[derive(Debug, Clone, Copy)]
pub struct InitOptions<'a> {
    pub adapter: &'a str,
    pub git_hooks: bool,
    pub sonar: &'a str,
    pub layout: &'a str,
    pub architecture: &'a str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SonarInitMode {
    Auto,
    Yes,
    No,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LayoutMode {
    Auto,
    Monorepo,
    MultiRepo,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkspaceLayout {
    Monorepo,
    MultiRepo,
}

impl SonarInitMode {
    fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "yes" | "y" | "true" => Ok(Self::Yes),
            "no" | "n" | "false" => Ok(Self::No),
            other => bail!("invalid `--sonar` mode `{other}`; expected `auto`, `yes`, or `no`"),
        }
    }
}

impl LayoutMode {
    fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "monorepo" | "mono" | "1" => Ok(Self::Monorepo),
            "multi-repo" | "multirepo" | "independent" | "2" => Ok(Self::MultiRepo),
            other => bail!(
                "invalid `--layout` mode `{other}`; expected `auto`, `monorepo`, or `multi-repo`"
            ),
        }
    }
}

/// Initializes `anvil` in `root` as single-repo, monorepo, or thin-umbrella multi-repo workspace.
pub fn execute(root: &Path, opts: InitOptions<'_>) -> Result<()> {
    ui::print_banner_if_tty("Workspace Initialization");
    fs::create_dir_all(root)
        .with_context(|| format!("failed to create target directory {}", root.display()))?;

    let explicit_adapter = AdapterTarget::parse_cli(opts.adapter)?;
    let sonar_mode = SonarInitMode::parse(opts.sonar)?;
    let layout_mode = LayoutMode::parse(opts.layout)?;
    let arch_mode = architecture::ArchMode::parse(opts.architecture)?;
    let use_tui = prompt::is_interactive_tui();
    let interactive = use_tui
        || io::stdin().is_terminal()
        || std::env::var_os("ANVIL_INIT_FORCE_PROMPT").is_some();
    let stdin = io::stdin();
    let mut reader = stdin.lock();

    let detection = stack_detector::detect_workspace(root)?;
    let child_repos: Vec<&DetectedRepo> =
        detection.repos.iter().filter(|r| r.rel_path != ".").collect();

    if child_repos.is_empty() {
        let root_sonar = if sonar_mode == SonarInitMode::Auto && use_tui {
            let cfg = resolve_single_sonar_tui(root)?;
            println!();
            Some(cfg)
        } else {
            prompt_sonar_for_repo_opt("root project", root, sonar_mode, &mut reader, interactive)?
        };
        let arch_plan =
            plan_single_architecture(root, arch_mode, interactive, use_tui, &mut reader)?;
        let _ = init_single_repo(
            root,
            explicit_adapter,
            opts.git_hooks,
            root_sonar,
            &arch_plan,
            "anvil initialized at",
        )?;
        return Ok(());
    }

    let layout = resolve_workspace_layout(
        root,
        &child_repos,
        layout_mode,
        &mut reader,
        interactive,
        use_tui,
    )?;
    let ctx = MultiInitCtx {
        root,
        child_repos: &child_repos,
        explicit_adapter,
        git_hooks: opts.git_hooks,
        sonar_mode,
        arch_mode,
        interactive,
        use_tui,
    };
    match layout {
        WorkspaceLayout::Monorepo => init_monorepo_root(&ctx, &mut reader),
        WorkspaceLayout::MultiRepo => init_thin_umbrella(&ctx, &mut reader),
    }
}

fn plan_single_architecture(
    root: &Path,
    mode: architecture::ArchMode,
    interactive: bool,
    use_tui: bool,
    reader: &mut dyn BufRead,
) -> Result<architecture::ArchPlan> {
    let candidate = architecture::Candidate::root("this project", root)?;
    let mut io = architecture::PromptIo { interactive, use_tui, reader };
    architecture::plan(mode, &[candidate], &mut io)
}

struct MultiInitCtx<'a> {
    root: &'a Path,
    child_repos: &'a [&'a DetectedRepo],
    explicit_adapter: Option<AdapterTarget>,
    git_hooks: bool,
    sonar_mode: SonarInitMode,
    arch_mode: architecture::ArchMode,
    interactive: bool,
    use_tui: bool,
}

fn resolve_workspace_layout(
    root: &Path,
    child_repos: &[&DetectedRepo],
    mode: LayoutMode,
    reader: &mut dyn BufRead,
    interactive: bool,
    use_tui: bool,
) -> Result<WorkspaceLayout> {
    match mode {
        LayoutMode::Monorepo => return Ok(WorkspaceLayout::Monorepo),
        LayoutMode::MultiRepo => return Ok(WorkspaceLayout::MultiRepo),
        LayoutMode::Auto => {}
    }
    let root_has_git = root.join(".git").exists();
    let any_child_has_git =
        child_repos.iter().any(|r| root.join(&r.rel_path).join(".git").exists());
    let default_layout = if root_has_git && !any_child_has_git {
        WorkspaceLayout::Monorepo
    } else {
        WorkspaceLayout::MultiRepo
    };
    if !interactive {
        return Ok(default_layout);
    }
    let names = child_repos.iter().map(|r| r.rel_path.as_str()).collect::<Vec<_>>().join(", ");
    if use_tui {
        return resolve_layout_tui(child_repos.len(), &names, default_layout);
    }
    resolve_layout_text(&names, default_layout, reader)
}

fn resolve_layout_tui(
    count: usize,
    names: &str,
    default_layout: WorkspaceLayout,
) -> Result<WorkspaceLayout> {
    let options = [
        SelectOption::new(
            "Independent repos in one folder",
            "thin umbrella root + .harness/ per repo",
        ),
        SelectOption::new("Monorepo", "single .harness/ and tasks.json at root"),
    ];
    let default_idx = usize::from(default_layout == WorkspaceLayout::Monorepo);
    let sub = format!("Detected {count} projects: {names}");
    match prompt::select(
        "How should Anvil manage this workspace?",
        "Workspace layout",
        Some(&sub),
        &options,
        default_idx,
    )? {
        PromptOutcome::Confirmed(1) => Ok(WorkspaceLayout::Monorepo),
        PromptOutcome::Confirmed(_) => Ok(WorkspaceLayout::MultiRepo),
        PromptOutcome::Cancelled => Err(ExitCodeError::silent(130).into()),
    }
}

fn resolve_layout_text(
    names: &str,
    default_layout: WorkspaceLayout,
    reader: &mut dyn BufRead,
) -> Result<WorkspaceLayout> {
    let def_num = if default_layout == WorkspaceLayout::Monorepo { "1" } else { "2" };
    println!("Multiple projects detected: {names}");
    println!("How should Anvil manage this workspace?");
    println!("  [1] Monorepo (single .harness/ and tasks.json at root)");
    println!("  [2] Independent repos in one folder (thin umbrella root + .harness/ per repo)");
    print!("Select layout [1/2] (default: {def_num}): ");
    io::stdout().flush()?;
    let mut ans = String::new();
    let _ = reader.read_line(&mut ans)?;
    match ans.trim().to_ascii_lowercase().as_str() {
        "1" | "m" | "monorepo" => Ok(WorkspaceLayout::Monorepo),
        "2" | "i" | "multi" | "multi-repo" => Ok(WorkspaceLayout::MultiRepo),
        _ => Ok(default_layout),
    }
}

fn init_monorepo_root(ctx: &MultiInitCtx<'_>, reader: &mut dyn BufRead) -> Result<()> {
    let mut sonar_configs: Vec<Option<SonarConfig>> =
        if ctx.sonar_mode == SonarInitMode::Auto && ctx.use_tui {
            let mut targets = Vec::with_capacity(1 + ctx.child_repos.len());
            targets.push((
                "monorepo root".to_owned(),
                ctx.root.to_path_buf(),
                "root workspace".to_owned(),
            ));
            for r in ctx.child_repos {
                targets.push((
                    r.rel_path.clone(),
                    ctx.root.join(&r.rel_path),
                    format_debug_list(&r.stacks),
                ));
            }
            let cfgs = resolve_multi_sonar_tui(&targets)?;
            println!();
            cfgs.into_iter().map(Some).collect()
        } else {
            let mut cfgs = Vec::with_capacity(1 + ctx.child_repos.len());
            cfgs.push(prompt_sonar_for_repo_opt(
                "monorepo root",
                ctx.root,
                ctx.sonar_mode,
                reader,
                ctx.interactive,
            )?);
            for r in ctx.child_repos {
                let sub_dir = ctx.root.join(&r.rel_path);
                let label = format!("subproject `{}`", r.rel_path);
                cfgs.push(prompt_sonar_for_repo_opt(
                    &label,
                    &sub_dir,
                    ctx.sonar_mode,
                    reader,
                    ctx.interactive,
                )?);
            }
            cfgs
        };
    let root_sonar = sonar_configs.drain(..1).next().flatten();
    let arch_plan = plan_monorepo_architecture(ctx, reader)?;
    let mut root_config = init_single_repo(
        ctx.root,
        ctx.explicit_adapter,
        ctx.git_hooks,
        root_sonar,
        &arch_plan,
        "anvil monorepo initialized at",
    )?;
    for (repo, sub_sonar) in ctx.child_repos.iter().zip(sonar_configs) {
        if let Some(sub_s) = sub_sonar
            && let Some(sub_entry) =
                root_config.subrepos.iter_mut().find(|s| s.path == repo.rel_path)
        {
            sub_entry.sonar_enabled = sub_s.enabled;
        }
    }
    root_config.save(ctx.root)
}

/// Architecture choices of a monorepo: its children, plus the root when it has its own `src/`.
fn plan_monorepo_architecture(
    ctx: &MultiInitCtx<'_>,
    reader: &mut dyn BufRead,
) -> Result<architecture::ArchPlan> {
    let root_config = AnvilConfig::load_or_default(ctx.root)?;
    let mut candidates = Vec::new();
    if ctx.root.join("src").is_dir() {
        candidates.push(architecture::Candidate::root("monorepo root", ctx.root)?);
    }
    for repo in ctx.child_repos {
        let dir = ctx.root.join(&repo.rel_path);
        candidates.push(architecture::Candidate::subrepo(
            &repo.rel_path,
            &repo.name,
            &dir,
            &root_config,
        ));
    }
    let mut io = architecture::PromptIo {
        interactive: ctx.interactive,
        use_tui: ctx.use_tui,
        reader: &mut *reader,
    };
    architecture::plan(ctx.arch_mode, &candidates, &mut io)
}

fn resolve_umbrella_sonar_configs(
    ctx: &MultiInitCtx<'_>,
    reader: &mut dyn BufRead,
) -> Result<Vec<Option<SonarConfig>>> {
    if ctx.sonar_mode == SonarInitMode::Auto && ctx.use_tui {
        let targets: Vec<(String, std::path::PathBuf, String)> = ctx
            .child_repos
            .iter()
            .map(|r| (r.rel_path.clone(), ctx.root.join(&r.rel_path), format_debug_list(&r.stacks)))
            .collect();
        let cfgs = resolve_multi_sonar_tui(&targets)?;
        println!();
        return Ok(cfgs.into_iter().map(Some).collect());
    }
    let mut cfgs = Vec::with_capacity(ctx.child_repos.len());
    for repo in ctx.child_repos {
        let sub_dir = ctx.root.join(&repo.rel_path);
        if !sub_dir.is_dir() {
            cfgs.push(None);
            continue;
        }
        let label = format!("subproject `{}`", repo.rel_path);
        cfgs.push(prompt_sonar_for_repo_opt(
            &label,
            &sub_dir,
            ctx.sonar_mode,
            reader,
            ctx.interactive,
        )?);
    }
    Ok(cfgs)
}

fn init_thin_umbrella(ctx: &MultiInitCtx<'_>, reader: &mut dyn BufRead) -> Result<()> {
    let sonar_configs = resolve_umbrella_sonar_configs(ctx, reader)?;

    let umbrella_list: Vec<UmbrellaSubrepo> = ctx
        .child_repos
        .iter()
        .map(|r| UmbrellaSubrepo {
            rel_path: r.rel_path.clone(),
            abs_path: ctx.root.join(&r.rel_path),
            stacks: r.stacks.clone(),
        })
        .collect();
    let (adapter_target, report) =
        sync::init_umbrella_root(ctx.root, ctx.explicit_adapter, &umbrella_list)?;
    println!(
        "{} {} {}",
        ui::green("✓"),
        ui::green("anvil thin umbrella initialized at"),
        ui::bold(&ctx.root.display().to_string())
    );
    ui::print_kv("layout", "multi-repo (thin root: no duplicate .harness/ or tasks.json)");
    ui::print_kv(
        "adapters",
        &format!("{adapter_target:?} (files_written={})", report.written_files.len()),
    );
    if ui::colors_enabled() {
        println!();
    }

    let mut plans = Vec::with_capacity(ctx.child_repos.len());
    for repo in ctx.child_repos {
        let sub_dir = ctx.root.join(&repo.rel_path);
        let candidate = architecture::Candidate::root(&repo.rel_path, &sub_dir)?;
        let mut io = architecture::PromptIo {
            interactive: ctx.interactive,
            use_tui: ctx.use_tui,
            reader: &mut *reader,
        };
        plans.push(architecture::plan(ctx.arch_mode, &[candidate], &mut io)?);
    }

    for ((repo, sub_sonar), arch_plan) in ctx.child_repos.iter().zip(sonar_configs).zip(&plans) {
        let sub_dir = ctx.root.join(&repo.rel_path);
        if !sub_dir.is_dir() {
            continue;
        }
        let _ = init_single_repo(
            &sub_dir,
            Some(adapter_target),
            ctx.git_hooks,
            sub_sonar,
            arch_plan,
            &format!("repo {} initialized at", repo.rel_path),
        )?;
    }
    Ok(())
}

fn resolve_single_sonar_tui(dir: &Path) -> Result<SonarConfig> {
    let props = parse_sonar_properties(dir);
    let options = [
        SelectOption::new("No", "disabled (default)"),
        SelectOption::new("Yes", "enable local SonarQube checks"),
    ];
    let choice = prompt::select(
        "Does this project use local SonarQube?",
        "Local SonarQube",
        Some("Opt-in only · runs only when enabled or via --sonar"),
        &options,
        0,
    )?;
    match choice {
        PromptOutcome::Cancelled => Err(ExitCodeError::silent(130).into()),
        PromptOutcome::Confirmed(0) => Ok(SonarConfig::default()),
        PromptOutcome::Confirmed(_) => {
            let host_override = prompt_host_tui_if_missing(
                props.as_ref().and_then(|p| p.host_url.as_ref()).is_none(),
            )?;
            Ok(hydrate_sonar_config(dir, props.as_ref(), host_override))
        }
    }
}

fn resolve_multi_sonar_tui(
    targets: &[(String, std::path::PathBuf, String)],
) -> Result<Vec<SonarConfig>> {
    let parsed: Vec<_> = targets.iter().map(|(_, dir, _)| parse_sonar_properties(dir)).collect();
    let options: Vec<SelectOption> = targets
        .iter()
        .zip(parsed.iter())
        .map(|((label, _, stack_hint), props)| {
            let hint = if props.is_some() {
                format!("{stack_hint} · sonar-project.properties")
            } else {
                stack_hint.clone()
            };
            SelectOption::new(label.as_str(), hint)
        })
        .collect();
    let defaults = vec![false; options.len()];
    let selected = match prompt::multi_select(
        "Which projects use local SonarQube?",
        "Local SonarQube",
        Some("Opt-in only · leave unchecked if none use SonarQube"),
        &options,
        &defaults,
    )? {
        PromptOutcome::Confirmed(flags) => flags,
        PromptOutcome::Cancelled => return Err(ExitCodeError::silent(130).into()),
    };
    let needs_host = selected
        .iter()
        .zip(parsed.iter())
        .any(|(on, props)| *on && props.as_ref().and_then(|p| p.host_url.as_ref()).is_none());
    let shared_host = prompt_host_tui_if_missing(needs_host)?;
    Ok(targets
        .iter()
        .zip(parsed.iter())
        .zip(selected)
        .map(|(((_, dir, _), props), on)| {
            if on {
                hydrate_sonar_config(dir, props.as_ref(), shared_host.clone())
            } else {
                SonarConfig::default()
            }
        })
        .collect())
}

fn prompt_host_tui_if_missing(needs_host: bool) -> Result<Option<String>> {
    if !needs_host {
        return Ok(None);
    }
    match prompt::text_input("Local SonarQube host URL", "SonarQube host", "http://localhost:9000")?
    {
        PromptOutcome::Confirmed(url) => Ok(Some(url.trim_end_matches('/').to_owned())),
        PromptOutcome::Cancelled => Err(ExitCodeError::silent(130).into()),
    }
}

fn prompt_sonar_for_repo_opt(
    label: &str,
    dir: &Path,
    mode: SonarInitMode,
    reader: &mut dyn BufRead,
    interactive: bool,
) -> Result<Option<SonarConfig>> {
    let props = parse_sonar_properties(dir);
    match mode {
        SonarInitMode::No => Ok(Some(SonarConfig::default())),
        SonarInitMode::Yes => Ok(Some(hydrate_sonar_config(dir, props.as_ref(), None))),
        SonarInitMode::Auto if !interactive => Ok(None),
        SonarInitMode::Auto => {
            print!("Does {label} use SonarQube? [y/N]: ");
            io::stdout().flush()?;
            let mut answer = String::new();
            let _ = reader.read_line(&mut answer)?;
            if !is_affirmative(&answer) {
                return Ok(Some(SonarConfig::default()));
            }
            let host_override = if props.as_ref().and_then(|p| p.host_url.as_ref()).is_none() {
                print!("  SonarQube host URL [http://localhost:9000]: ");
                io::stdout().flush()?;
                let mut url_line = String::new();
                let _ = reader.read_line(&mut url_line)?;
                let trimmed = url_line.trim();
                (!trimmed.is_empty()).then(|| trimmed.trim_end_matches('/').to_owned())
            } else {
                None
            };
            Ok(Some(hydrate_sonar_config(dir, props.as_ref(), host_override)))
        }
    }
}

fn hydrate_sonar_config(
    dir: &Path,
    props: Option<&anvil_core::sonar::SonarProperties>,
    host_override: Option<String>,
) -> SonarConfig {
    let fallback_key = dir.file_name().and_then(|n| n.to_str()).map(str::to_owned);
    SonarConfig {
        enabled: true,
        host_url: host_override
            .or_else(|| props.and_then(|p| p.host_url.clone()))
            .or_else(|| Some("http://localhost:9000".to_owned())),
        project_key: props.and_then(|p| p.project_key.clone()).or(fallback_key),
        ..SonarConfig::default()
    }
}

fn is_affirmative(raw: &str) -> bool {
    matches!(raw.trim().to_ascii_lowercase().as_str(), "y" | "yes" | "s" | "si" | "sí")
}

fn init_single_repo(
    dir: &Path,
    explicit_adapter: Option<AdapterTarget>,
    git_hooks: bool,
    sonar_override: Option<SonarConfig>,
    arch_plan: &architecture::ArchPlan,
    banner: &str,
) -> Result<AnvilConfig> {
    let report = sync::init_repo_with_adapter(dir, explicit_adapter)?;
    let mut config = AnvilConfig::load_or_default(dir)?;
    if let Some(sonar) = sonar_override {
        config.sonar = sonar;
        config.save(dir)?;
    }
    let architecture_line = architecture::apply(dir, &mut config, arch_plan)?;
    let prereqs_line = prereqs::detect_and_store(dir, &mut config)?;

    let merge_driver_status = if dir.join(".git").exists() {
        install_git_merge_driver(dir)?;
        "installed"
    } else {
        "skipped (no .git)"
    };
    let pre_commit_status = if git_hooks {
        if crate::commands::hook::git_install::install_pre_commit_hook(dir)? {
            "installed"
        } else {
            "unchanged/skipped"
        }
    } else {
        "not requested"
    };

    let engine = CodeGraphEngine::build(dir, config.clone())?;
    let graph_bin = engine.save()?;
    let tasks = build_root_tasks_json(dir)?;
    print_init_summary(InitSummary {
        dir,
        config: &config,
        git_hooks,
        pre_commit_status,
        merge_driver_status,
        banner,
        nodes: engine.graph.nodes.len(),
        edges: engine.graph.edges.len(),
        graph_bin: &graph_bin,
        task_count: tasks.task_count,
        files_written: report.written_files.len(),
        prereqs: &prereqs_line,
        architecture: &architecture_line,
    });
    Ok(config)
}

struct InitSummary<'a> {
    dir: &'a Path,
    config: &'a AnvilConfig,
    git_hooks: bool,
    pre_commit_status: &'a str,
    merge_driver_status: &'a str,
    banner: &'a str,
    nodes: usize,
    edges: usize,
    graph_bin: &'a Path,
    task_count: usize,
    files_written: usize,
    prereqs: &'a str,
    architecture: &'a str,
}

fn print_init_summary(s: InitSummary<'_>) {
    let stacks_str = format_debug_list(&s.config.stacks);
    let tools_str = format_debug_list(&s.config.tools);
    let prefix = if ui::colors_enabled() { "✓ " } else { "" };
    println!(
        "{}{} {}",
        ui::green(prefix),
        ui::green(s.banner),
        ui::bold(&s.dir.display().to_string())
    );
    ui::print_kv("stacks", &ui::cyan(&stacks_str));
    ui::print_kv("tools", &ui::cyan(&tools_str));
    ui::print_kv("adapters", &format!("{:?}", s.config.adapters));
    ui::print_kv("merge_driver", s.merge_driver_status);
    if s.git_hooks {
        ui::print_kv("pre_commit_hook", s.pre_commit_status);
    }
    let sonar_label = if s.config.sonar.enabled {
        format!(
            "enabled (host={})",
            s.config.sonar.host_url.as_deref().unwrap_or("http://localhost:9000")
        )
    } else {
        "disabled".to_owned()
    };
    ui::print_kv("sonar", &sonar_label);
    ui::print_kv(
        "graph",
        &format!(
            "{} nodes, {} edges  {}",
            ui::bold(&s.nodes.to_string()),
            ui::bold(&s.edges.to_string()),
            ui::dim(&format!("({})", s.graph_bin.display()))
        ),
    );
    ui::print_kv("architecture", s.architecture);
    ui::print_kv("prereqs", s.prereqs);
    ui::print_kv("summary", &format!("tasks={}  files_written={}", s.task_count, s.files_written));
    if ui::colors_enabled() {
        println!();
    }
}

fn format_debug_list<T: std::fmt::Debug>(items: &[T]) -> String {
    if items.is_empty() {
        return "none".to_owned();
    }
    items.iter().map(|item| format!("{item:?}")).collect::<Vec<_>>().join(", ")
}
