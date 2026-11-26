//! Local QA tool probing and driver option catalog builders.

use std::env;
use std::fs;
use std::path::Path;

use anvil_core::config::{AnvilConfig, ToolKind};

use super::ToolAvailability;
use crate::model::DriverOption;

pub(super) fn probe_tools(repo_root: &Path, config: &AnvilConfig) -> ToolAvailability {
    let has_roborazzi_tool = config.tools.contains(&ToolKind::Roborazzi)
        || config.subrepos.iter().any(|sub| sub.tools.contains(&ToolKind::Roborazzi))
        || repo_mentions_roborazzi(repo_root);

    ToolAvailability {
        playwright: has_playwright(repo_root),
        chrome: command_in_path("chrome")
            || command_in_path("chromium")
            || command_in_path("google-chrome")
            || command_in_path("msedge"),
        adb: command_in_path("adb"),
        roborazzi: has_roborazzi_tool,
        maestro: command_in_path("maestro"),
    }
}

pub(super) fn build_web_options(availability: ToolAvailability) -> Vec<DriverOption> {
    vec![
        DriverOption {
            id: "playwright".to_owned(),
            label: "Playwright (Headless Chromium)".to_owned(),
            description: "Automated desktop (1440x900) & mobile (390x844) DOM audit + screenshots."
                .to_owned(),
            available: availability.playwright,
            recommended: availability.playwright,
        },
        DriverOption {
            id: "chrome_dev_tools".to_owned(),
            label: "Chrome DevTools MCP / CDP".to_owned(),
            description: "Connect to a Chrome DevTools / CDP session for live DOM assertions."
                .to_owned(),
            available: availability.chrome,
            recommended: !availability.playwright && availability.chrome,
        },
        DriverOption {
            id: "browser".to_owned(),
            label: "Integrated Browser".to_owned(),
            description: "Run dom_audit.js manually or via the host IDE browser tool.".to_owned(),
            available: true,
            recommended: false,
        },
        DriverOption {
            id: "skip".to_owned(),
            label: "Skip Web Visual QA".to_owned(),
            description: "Disable web visual QA prompts for this repository.".to_owned(),
            available: true,
            recommended: false,
        },
    ]
}

pub(super) fn build_mobile_options(availability: ToolAvailability) -> Vec<DriverOption> {
    vec![
        DriverOption {
            id: "roborazzi".to_owned(),
            label: "Roborazzi (Kotlin/Compose Screenshot Tests)".to_owned(),
            description: "Fast JVM screenshot diffing for Android Compose UI without an emulator."
                .to_owned(),
            available: availability.roborazzi,
            recommended: availability.roborazzi,
        },
        DriverOption {
            id: "adb_mobile_mcp".to_owned(),
            label: "Android ADB + uiautomator / mobile-mcp".to_owned(),
            description: "Inspect live view tree bounds, logcat crashes/ANRs, and emulator UI."
                .to_owned(),
            available: availability.adb,
            recommended: !availability.roborazzi && availability.adb,
        },
        DriverOption {
            id: "maestro".to_owned(),
            label: "Maestro".to_owned(),
            description: "Run declarative mobile UI flows.".to_owned(),
            available: availability.maestro,
            recommended: false,
        },
        DriverOption {
            id: "skip".to_owned(),
            label: "Skip Mobile Visual QA".to_owned(),
            description: "Disable mobile visual QA prompts for this repository.".to_owned(),
            available: true,
            recommended: false,
        },
    ]
}

fn has_playwright(repo_root: &Path) -> bool {
    let local_bin = if cfg!(windows) {
        repo_root.join("node_modules/.bin/playwright.cmd")
    } else {
        repo_root.join("node_modules/.bin/playwright")
    };
    local_bin.exists() || command_in_path("playwright")
}

fn repo_mentions_roborazzi(repo_root: &Path) -> bool {
    for rel in ["build.gradle.kts", "app/build.gradle.kts", "gradle/libs.versions.toml"] {
        let candidate = repo_root.join(rel);
        if let Ok(text) = fs::read_to_string(candidate)
            && text.to_ascii_lowercase().contains("roborazzi")
        {
            return true;
        }
    }
    false
}

fn command_in_path(bin: &str) -> bool {
    let Some(path_var) = env::var_os("PATH") else {
        return false;
    };
    let exts: &[&str] = if cfg!(windows) { &["", ".exe", ".cmd", ".bat"] } else { &[""] };
    for dir in env::split_paths(&path_var) {
        for ext in exts {
            if dir.join(format!("{bin}{ext}")).is_file() {
                return true;
            }
        }
    }
    false
}
