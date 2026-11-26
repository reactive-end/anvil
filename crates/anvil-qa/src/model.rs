//! Shared data structures for hybrid Web and Mobile visual QA.

use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// UI surface(s) detected in a repository or multi-repo workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiPlatform {
    Web,
    Mobile,
    Both,
    None,
}

impl UiPlatform {
    /// Returns true when web UI assertions apply.
    pub fn has_web(self) -> bool {
        matches!(self, Self::Web | Self::Both)
    }

    /// Returns true when mobile UI assertions apply.
    pub fn has_mobile(self) -> bool {
        matches!(self, Self::Mobile | Self::Both)
    }
}

/// Objective visual or runtime QA violation categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QaViolationKind {
    HorizontalOverflow,
    ClippedText,
    OverlappingInteractive,
    ZeroSizeControl,
    BrokenImage,
    ConsoleError,
    PageError,
    MobileTruncatedOrOutOfViewport,
    MobileOverlappingInteractive,
    MobileFatalException,
    MobileAnr,
    RoborazziDiff,
    Godot3dMissingCamera,
    Godot3dMissingLighting,
    GodotMissingCollisionShape,
}

impl QaViolationKind {
    /// Stable compact identifier used in compressed LLM reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HorizontalOverflow => "horizontal_overflow",
            Self::ClippedText => "clipped_text",
            Self::OverlappingInteractive => "overlapping_interactive",
            Self::ZeroSizeControl => "zero_size_control",
            Self::BrokenImage => "broken_image",
            Self::ConsoleError => "console_error",
            Self::PageError => "page_error",
            Self::MobileTruncatedOrOutOfViewport => "mobile_truncated_or_out_of_viewport",
            Self::MobileOverlappingInteractive => "mobile_overlapping_interactive",
            Self::MobileFatalException => "mobile_fatal_exception",
            Self::MobileAnr => "mobile_anr",
            Self::RoborazziDiff => "roborazzi_diff",
            Self::Godot3dMissingCamera => "godot-3d-missing-camera",
            Self::Godot3dMissingLighting => "godot-3d-missing-lighting",
            Self::GodotMissingCollisionShape => "godot-missing-collision-shape",
        }
    }
}

impl fmt::Display for QaViolationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Integer pixel bounding rectangle for a UI element or violation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// A single objective visual, DOM, view-tree, or runtime failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QaViolation {
    pub kind: QaViolationKind,
    pub selector: String,
    pub message: String,
    pub rect: Option<Rect>,
    pub viewport: Option<String>,
}

/// Capture and assertion results for a single web viewport pass.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewportCapture {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub screenshot_path: Option<PathBuf>,
    pub violations: Vec<QaViolation>,
    pub console_errors: Vec<String>,
    pub page_errors: Vec<String>,
}

impl ViewportCapture {
    /// Returns true if no DOM violations or runtime errors were recorded.
    pub fn passed(&self) -> bool {
        self.violations.is_empty() && self.console_errors.is_empty() && self.page_errors.is_empty()
    }

    /// Total violation count including console and page errors without double-counting.
    pub fn total_violations(&self) -> usize {
        let dom_count = self
            .violations
            .iter()
            .filter(|v| {
                !matches!(v.kind, QaViolationKind::ConsoleError | QaViolationKind::PageError)
            })
            .count();
        let console_in_v =
            self.violations.iter().filter(|v| v.kind == QaViolationKind::ConsoleError).count();
        let page_in_v =
            self.violations.iter().filter(|v| v.kind == QaViolationKind::PageError).count();
        dom_count
            + console_in_v.max(self.console_errors.len())
            + page_in_v.max(self.page_errors.len())
    }
}

/// Complete before/after visual QA report across detected platforms.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QaReport {
    pub platform: UiPlatform,
    pub passed: bool,
    pub before: Vec<ViewportCapture>,
    pub after: Vec<ViewportCapture>,
    pub mobile_violations: Vec<QaViolation>,
    pub notes: Vec<String>,
}

/// A selectable QA tool/driver presented during interactive onboarding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriverOption {
    pub id: String,
    pub label: String,
    pub description: String,
    pub available: bool,
    pub recommended: bool,
}

/// Structured prompt emitted when `qa_driver` is `Unconfigured` for an active UI platform.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingPrompt {
    pub platform: UiPlatform,
    pub web_needed: bool,
    pub mobile_needed: bool,
    pub web_options: Vec<DriverOption>,
    pub mobile_options: Vec<DriverOption>,
}

/// Command specification returned by runner builders for deterministic testing and execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunnerCommand {
    pub program: String,
    pub args: Vec<String>,
}
