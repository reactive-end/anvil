//! `anvil ratchet`: governed linter ratchet candidate logging, review, and promotion.

use std::fs;
use std::path::Path;

use anvil_core::ratchet::{
    CandidateInput, CandidateOptions, evaluate_candidate, list_candidates, list_rules,
    log_candidate_with_options, promote_candidate_with_severity, test_candidate,
};
use anvil_core::traits::Severity;
use anyhow::Result;

use crate::cli::subcmds::RatchetSubcmd;
use crate::commands::ExitCodeError;

/// Executes `anvil ratchet <log|list|review|test|promote>`.
pub fn execute(root: &Path, sub: RatchetSubcmd) -> Result<()> {
    match sub {
        RatchetSubcmd::Log {
            rule,
            description,
            pattern,
            lang,
            glob,
            severity,
            high_impact,
            runtime_ms,
            example,
        } => {
            let sev = severity.as_deref().map(parse_severity).transpose()?;
            let saved = log_candidate_with_options(
                root,
                CandidateInput {
                    rule_name: rule,
                    description,
                    pattern,
                    languages: lang,
                    high_impact,
                    deterministic: true,
                    estimated_runtime_ms: runtime_ms,
                    example,
                },
                CandidateOptions { file_globs: glob, severity: sev, ..Default::default() },
            )?;
            println!(
                "logged candidate id={} rule={} recurrence={}",
                saved.id, saved.rule_name, saved.recurrence
            );
        }
        RatchetSubcmd::List => cmd_list(root)?,
        RatchetSubcmd::Review { id, samples } => cmd_review(root, &id, &samples)?,
        RatchetSubcmd::Test { id } => cmd_test(root, &id)?,
        RatchetSubcmd::Promote { id, severity } => {
            let sev = severity.as_deref().map(parse_severity).transpose()?;
            let promoted = promote_candidate_with_severity(root, &id, sev)?;
            println!(
                "promoted rule {} (`{}`) severity={:?} at recurrence={}",
                promoted.id,
                promoted.rule_name,
                promoted.severity.unwrap_or(Severity::Warning),
                promoted.recurrence_at_promotion
            );
        }
    }
    Ok(())
}

fn cmd_list(root: &Path) -> Result<()> {
    let candidates = list_candidates(root)?;
    let rules = list_rules(root)?;
    println!("candidates ({}):", candidates.len());
    for c in candidates {
        println!(
            "  {} [{}] recurrence={} high_impact={} fp_checked={} fps={}",
            c.id, c.rule_name, c.recurrence, c.high_impact, c.fp_checked, c.false_positives_found
        );
    }
    println!("promoted rules ({}):", rules.len());
    for r in rules {
        println!("  {} [{}] {}", r.id, r.rule_name, r.description);
    }
    Ok(())
}

fn cmd_review(root: &Path, id: &str, raw_samples: &[String]) -> Result<()> {
    let loaded: Vec<String> = raw_samples
        .iter()
        .map(|s| {
            let path = Path::new(s);
            if path.is_file() {
                fs::read_to_string(path).unwrap_or_else(|_| s.clone())
            } else {
                s.clone()
            }
        })
        .collect();
    let sample_refs: Vec<&str> = loaded.iter().map(String::as_str).collect();
    let samples_opt = (!sample_refs.is_empty()).then_some(sample_refs.as_slice());

    let eval = evaluate_candidate(root, id, samples_opt)?;
    println!(
        "ratchet review `{}`: eligible={} (recurrence/impact={}, fast={}, zero_fp={}, {}ms)",
        eval.id,
        eval.eligible,
        eval.filter_recurrence_or_impact,
        eval.filter_deterministic_fast,
        eval.filter_zero_false_positives,
        eval.measured_runtime_ms
    );
    for reason in &eval.reasons {
        println!("  blocker: {reason}");
    }
    if !eval.eligible {
        return Err(ExitCodeError::silent(1).into());
    }
    Ok(())
}

fn cmd_test(root: &Path, id: &str) -> Result<()> {
    let report = test_candidate(root, id)?;
    println!(
        "ratchet test `{}` ({}): scanned={} matches={} elapsed={}ms under_budget={}",
        report.id,
        report.rule_name,
        report.files_scanned,
        report.match_count,
        report.elapsed_ms,
        report.under_budget
    );
    for m in &report.sample_matches {
        println!("  {}:{}: {}", m.file, m.line, m.snippet);
    }
    Ok(())
}

fn parse_severity(raw: &str) -> Result<Severity> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "error" | "err" => Ok(Severity::Error),
        "warn" | "warning" => Ok(Severity::Warning),
        "info" => Ok(Severity::Info),
        _ => anyhow::bail!("invalid severity `{raw}`; expected error|warn|info"),
    }
}
