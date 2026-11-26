//! Fast `adb logcat` scanner detecting `FATAL EXCEPTION` and `ANR in` markers.

use std::sync::OnceLock;

use regex::Regex;

use crate::model::{QaViolation, QaViolationKind};

static FATAL_RE: OnceLock<Regex> = OnceLock::new();
static ANR_RE: OnceLock<Regex> = OnceLock::new();

/// Scans `adb logcat` output for `FATAL EXCEPTION` and `ANR in` markers using `OnceLock<Regex>`.
pub fn scan_logcat(logcat: &str) -> Vec<QaViolation> {
    let fatal_re = FATAL_RE.get_or_init(|| {
        Regex::new(r"(?i)FATAL EXCEPTION(?::\s*(.+))?").unwrap_or_else(|_| unreachable!())
    });
    let anr_re =
        ANR_RE.get_or_init(|| Regex::new(r"ANR in\s+([^\s(]+)").unwrap_or_else(|_| unreachable!()));

    let mut violations = Vec::new();
    for (idx, line) in logcat.lines().enumerate() {
        let trimmed = line.trim();
        if let Some(caps) = fatal_re.captures(trimmed) {
            let detail = caps.get(1).map_or("main", |m| m.as_str().trim());
            violations.push(QaViolation {
                kind: QaViolationKind::MobileFatalException,
                selector: format!("logcat:L{}", idx + 1),
                message: format!("FATAL EXCEPTION ({detail})"),
                rect: None,
                viewport: Some("android".to_owned()),
            });
        } else if let Some(caps) = anr_re.captures(trimmed) {
            let proc_name = caps.get(1).map_or("unknown", |m| m.as_str());
            violations.push(QaViolation {
                kind: QaViolationKind::MobileAnr,
                selector: format!("logcat:L{}", idx + 1),
                message: format!("ANR in {proc_name}"),
                rect: None,
                viewport: Some("android".to_owned()),
            });
        }
    }
    violations
}
