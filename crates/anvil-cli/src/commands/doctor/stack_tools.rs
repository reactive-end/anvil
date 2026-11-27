//! Doctor check that the CLI tools of the detected stacks are on `PATH` or in the local tool folders.

use std::path::{Path, PathBuf};

use anvil_core::config::{AnvilConfig, StackKind, ToolKind};
use anvil_core::fs_util::{resolve_godot_bin, resolve_local_or_path_bin, resolve_php_bin};

use crate::ui;

pub(super) fn check_stack_tools(root: &Path, config: &AnvilConfig) {
    let mut bins = vec!["git", "gh"];
    if workspace_has_php(config) {
        bins.push("php");
    }
    for tool in config.tools.iter().chain(config.subrepos.iter().flat_map(|s| s.tools.iter())) {
        if let Some(bin) = tool_kind_bin(*tool)
            && !bins.contains(&bin)
        {
            bins.push(bin);
        }
    }
    let search_dirs: Vec<PathBuf> = std::iter::once(root.to_path_buf())
        .chain(config.subrepos.iter().map(|s| root.join(&s.path)))
        .filter(|d| d.is_dir())
        .collect();
    let missing: Vec<&str> = bins
        .into_iter()
        .filter(|b| match *b {
            "tsc" => !search_dirs.iter().any(|d| {
                resolve_local_or_path_bin(d, "tsgo").is_some()
                    || resolve_local_or_path_bin(d, "tsc").is_some()
            }),
            "php" => resolve_php_bin().is_none(),
            "godot" => resolve_godot_bin().is_none(),
            other => !search_dirs.iter().any(|d| resolve_local_or_path_bin(d, other).is_some()),
        })
        .collect();
    if missing.is_empty() {
        ui::print_status_row(
            &ui::ok_badge(),
            "tools",
            "all detected CLI tools found in local/PATH",
        );
    } else {
        ui::print_status_row(
            &ui::warn_badge(),
            "tools",
            &format!("not on local/PATH: {}", ui::yellow(&missing.join(", "))),
        );
    }
}

fn workspace_has_php(config: &AnvilConfig) -> bool {
    config
        .stacks
        .iter()
        .chain(config.subrepos.iter().flat_map(|s| s.stacks.iter()))
        .any(|s| matches!(s, StackKind::Php | StackKind::Laravel))
}

fn tool_kind_bin(tool: ToolKind) -> Option<&'static str> {
    match tool {
        ToolKind::Tsc => Some("tsc"),
        ToolKind::VueTsc => Some("vue-tsc"),
        ToolKind::Eslint => Some("eslint"),
        ToolKind::Biome => Some("biome"),
        ToolKind::Ruff => Some("ruff"),
        ToolKind::Pyright => Some("pyright"),
        ToolKind::Mypy => Some("mypy"),
        ToolKind::Ktlint => Some("ktlint"),
        ToolKind::Detekt => Some("detekt"),
        ToolKind::DartAnalyze => Some("dart"),
        ToolKind::Vitest => Some("vitest"),
        ToolKind::Jest => Some("jest"),
        ToolKind::Pytest => Some("pytest"),
        ToolKind::Cargo => Some("cargo"),
        ToolKind::Composer => Some("composer"),
        ToolKind::PhpStan => Some("phpstan"),
        ToolKind::Psalm => Some("psalm"),
        ToolKind::Pint => Some("pint"),
        ToolKind::PhpCsFixer => Some("php-cs-fixer"),
        ToolKind::PhpUnit => Some("phpunit"),
        ToolKind::Pest => Some("pest"),
        ToolKind::Godot => Some("godot"),
        ToolKind::GdLint => Some("gdlint"),
        ToolKind::Gradle
        | ToolKind::Roborazzi
        | ToolKind::SonarQube
        | ToolKind::GdUnit4
        | ToolKind::Gut => None,
    }
}
