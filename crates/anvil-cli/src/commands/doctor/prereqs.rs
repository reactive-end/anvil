//! Doctor's view of `anvil prereqs`: a one-line summary, or the full table with `--prereqs`.

use std::path::Path;

use anvil_core::config::AnvilConfig;

use crate::commands::prereqs::{counts, run_check};
use crate::ui;

/// Prints the summary row; with `full`, also the table, and required failures count as critical.
pub(super) fn check(root: &Path, config: &AnvilConfig, full: bool, failures: &mut usize) {
    let report = match run_check(root, config, None) {
        Ok(report) => report,
        Err(err) => {
            ui::print_status_row(&ui::warn_badge(), "prereqs", &format!("not checked: {err:#}"));
            return;
        }
    };
    let badge = if report.ok { ui::ok_badge() } else { ui::warn_badge() };
    let hint = if report.ok { "" } else { "  (run `anvil prereqs check` for details)" };
    ui::print_status_row(&badge, "prereqs", &format!("{}{hint}", counts(&report)));
    if full {
        crate::commands::prereqs::print_report(&report);
        *failures += report.failures();
    }
}
