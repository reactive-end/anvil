//! UI platform detection, local QA tool probing, and driver onboarding persistence.

mod options;

use std::path::Path;

use anyhow::Result;
use anvil_core::config::{AnvilConfig, MobileQaDriver, StackKind, WebQaDriver};

use crate::model::{OnboardingPrompt, UiPlatform};
use options::{build_mobile_options, build_web_options, probe_tools};

/// Local availability flags for supported Web and Mobile QA tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ToolAvailability {
    pub playwright: bool,
    pub chrome: bool,
    pub adb: bool,
    pub roborazzi: bool,
    pub maestro: bool,
}

impl ToolAvailability {
    /// Probes PATH and repository manifests for installed QA tooling.
    pub fn probe(repo_root: &Path, config: &AnvilConfig) -> Self {
        probe_tools(repo_root, config)
    }
}

/// Detects the active UI platform(s) from `AnvilConfig::stacks` and `subrepos`.
pub fn detect_ui_platform(config: &AnvilConfig) -> UiPlatform {
    let mut has_web = false;
    let mut has_mobile = false;

    for stack in config
        .stacks
        .iter()
        .chain(config.subrepos.iter().flat_map(|s| s.stacks.iter()))
    {
        match stack {
            StackKind::React
            | StackKind::Vite
            | StackKind::NextJs
            | StackKind::Astro
            | StackKind::Angular
            | StackKind::Laravel => has_web = true,
            StackKind::ExpoReactNative | StackKind::KotlinAndroid | StackKind::FlutterDart => {
                has_mobile = true;
            }
            StackKind::TypeScript
            | StackKind::NestJs
            | StackKind::Python
            | StackKind::Rust
            | StackKind::Php => {}
        }
    }

    match (has_web, has_mobile) {
        (true, true) => UiPlatform::Both,
        (true, false) => UiPlatform::Web,
        (false, true) => UiPlatform::Mobile,
        (false, false) => UiPlatform::None,
    }
}

/// Builds an onboarding prompt by probing local tools if any active platform is `Unconfigured`.
pub fn build_onboarding_prompt(
    repo_root: &Path,
    config: &AnvilConfig,
) -> Option<OnboardingPrompt> {
    let availability = ToolAvailability::probe(repo_root, config);
    build_onboarding_prompt_with(config, availability)
}

/// Builds an onboarding prompt using pre-probed tool availability.
pub fn build_onboarding_prompt_with(
    config: &AnvilConfig,
    availability: ToolAvailability,
) -> Option<OnboardingPrompt> {
    let platform = detect_ui_platform(config);
    let web_needed = platform.has_web() && config.qa_driver.web == WebQaDriver::Unconfigured;
    let mobile_needed =
        platform.has_mobile() && config.qa_driver.mobile == MobileQaDriver::Unconfigured;

    if !web_needed && !mobile_needed {
        return None;
    }

    let web_options = if web_needed {
        build_web_options(availability)
    } else {
        Vec::new()
    };
    let mobile_options = if mobile_needed {
        build_mobile_options(availability)
    } else {
        Vec::new()
    };

    Some(OnboardingPrompt {
        platform,
        web_needed,
        mobile_needed,
        web_options,
        mobile_options,
    })
}

/// Persists a selected web QA driver into `.harness/config.json`.
pub fn persist_web_driver(repo_root: &Path, driver: WebQaDriver) -> Result<AnvilConfig> {
    let mut config = AnvilConfig::load_or_default(repo_root)?;
    config.qa_driver.web = driver;
    config.save(repo_root)?;
    Ok(config)
}

/// Persists a selected mobile QA driver into `.harness/config.json`.
pub fn persist_mobile_driver(repo_root: &Path, driver: MobileQaDriver) -> Result<AnvilConfig> {
    let mut config = AnvilConfig::load_or_default(repo_root)?;
    config.qa_driver.mobile = driver;
    config.save(repo_root)?;
    Ok(config)
}

/// Parses a `DriverOption::id` into a [`WebQaDriver`].
pub fn parse_web_driver(id: &str) -> Option<WebQaDriver> {
    match id {
        "chrome_dev_tools" => Some(WebQaDriver::ChromeDevTools),
        "playwright" => Some(WebQaDriver::Playwright),
        "browser" => Some(WebQaDriver::Browser),
        "skip" => Some(WebQaDriver::Skip),
        _ => None,
    }
}

/// Parses a `DriverOption::id` into a [`MobileQaDriver`].
pub fn parse_mobile_driver(id: &str) -> Option<MobileQaDriver> {
    match id {
        "adb_mobile_mcp" => Some(MobileQaDriver::AdbMobileMcp),
        "roborazzi" => Some(MobileQaDriver::Roborazzi),
        "maestro" => Some(MobileQaDriver::Maestro),
        "skip" => Some(MobileQaDriver::Skip),
        _ => None,
    }
}
