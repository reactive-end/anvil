//! `anvil review`: review packets, approvals, and problems found after `anvil check` was green.

use std::path::Path;

use anvil_core::review_gate;
use anvil_core::stats::{self, NewFinding, ReviewEntry};
use anvil_tasks::review;
use anyhow::Result;

use crate::cli::subcmds::ReviewSubcmd;
use crate::ui;

/// Executes `anvil review <subcommand>`.
pub fn execute(root: &Path, sub: ReviewSubcmd) -> Result<()> {
    match sub {
        ReviewSubcmd::Log { description, category, severity, file, source } => {
            cmd_log(root, NewFinding { description, category, severity, source, file })
        }
        ReviewSubcmd::List { since } => cmd_list(root, since),
        ReviewSubcmd::Packet { task_id } => {
            print!("{}", review::packet(root, task_id.as_deref())?.text);
            Ok(())
        }
        ReviewSubcmd::Approve { task_id, summary } => {
            review_gate::approve(root, &task_id, &summary)?;
            println!("approved {task_id}: {}", summary.trim());
            Ok(())
        }
    }
}

fn cmd_log(root: &Path, finding: NewFinding) -> Result<()> {
    let logged = stats::log_review(root, finding)?;
    let entry = &logged.entry;
    let when = if entry.after_green { "after a green check" } else { "no green check before it" };
    println!(
        "recorded {} {} finding in `{}` ({when})",
        entry.severity,
        entry.source,
        ui::cyan(&entry.category)
    );
    if logged.new_category {
        let known = if logged.known_categories.is_empty() {
            "none yet".to_owned()
        } else {
            logged.known_categories.join(", ")
        };
        println!("new category; existing categories: {known}");
    }
    Ok(())
}

fn cmd_list(root: &Path, since: Option<u32>) -> Result<()> {
    let entries = stats::list_reviews(root, since)?;
    if entries.is_empty() {
        println!("no review findings recorded");
        return Ok(());
    }
    for entry in &entries {
        println!("{}", format_entry(entry));
    }
    Ok(())
}

fn format_entry(entry: &ReviewEntry) -> String {
    let green = if entry.after_green { " after-green" } else { "" };
    let file = entry.file.as_deref().map_or(String::new(), |f| format!(" {f}"));
    format!(
        "{} [{}] {} ({}){green}{file}: {}",
        entry.at.format("%Y-%m-%d"),
        entry.severity,
        entry.category,
        entry.source,
        entry.description
    )
}
