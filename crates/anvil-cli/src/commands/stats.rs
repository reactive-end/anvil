//! `anvil stats`: inspect or reset local hook latency, cache hits, token savings, and rule hits.

use std::path::Path;

use anyhow::Result;
use anvil_core::circuit_breaker::CircuitBreaker;
use anvil_core::config::AnvilConfig;
use anvil_core::stats::{AnvilStats, load_stats, percentile_ms, reset_stats};

use crate::ui;

/// Executes `anvil stats [--reset] [--trajectories]`.
pub fn execute(root: &Path, reset: bool, trajectories: bool) -> Result<()> {
    if reset {
        reset_stats(root)?;
        println!("{}", ui::green("stats reset"));
        return Ok(());
    }

    ui::print_banner_if_tty("Telemetry & Token Savings");
    ui::print_section("Session Telemetry");

    let st = load_stats(root)?;
    let avg_ms = if st.hook_calls > 0 {
        st.total_hook_ms as f64 / st.hook_calls as f64
    } else {
        0.0
    };
    let total_tokens_saved = st
        .tokens_saved_compressor
        .saturating_add(st.tokens_saved_symbol_read)
        .saturating_add(st.tokens_saved_outline)
        .saturating_add(st.tokens_saved_code_mode);

    println!("{}", ui::accent("anvil stats:"));
    ui::print_kv(
        "hooks:",
        &format!(
            "calls={} avg_ms={} total_ms={}",
            ui::cyan(&st.hook_calls.to_string()),
            ui::cyan(&format!("{avg_ms:.2}")),
            st.total_hook_ms
        ),
    );
    ui::print_kv("check_cache:", &format!("hits={}", ui::green(&st.check_cache_hits.to_string())));
    let code_mode_suffix = if st.code_mode_calls > 0 || trajectories {
        format!(", code_mode={}", st.tokens_saved_code_mode)
    } else {
        String::new()
    };
    ui::print_kv(
        "tokens_saved:",
        &format!(
            "total={} (compressor={}, symbol_read={}, outline={}{code_mode_suffix})",
            ui::green(&total_tokens_saved.to_string()),
            st.tokens_saved_compressor,
            st.tokens_saved_symbol_read,
            st.tokens_saved_outline,
        ),
    );
    ui::print_kv(
        "token_guard:",
        &format!(
            "blocks={} warnings={} tokens_avoided={}",
            ui::yellow(&st.token_guard_blocks.to_string()),
            st.token_guard_warnings,
            st.tokens_avoided_token_guard
        ),
    );
    print_effectiveness(&st);
    if trajectories {
        print_trajectory_section(root, &st);
    }
    print_top_rule_hits(&st);
    Ok(())
}

/// Reports reach and reduction so savings can be judged against how often a layer is used.
fn print_effectiveness(st: &AnvilStats) {
    let reduction = if st.tokens_raw_compressor > 0 {
        st.tokens_saved_compressor.min(st.tokens_raw_compressor) as f64 * 100.0
            / st.tokens_raw_compressor as f64
    } else {
        0.0
    };
    ui::print_kv(
        "compressor:",
        &format!(
            "runs={} raw_tokens={} reduction={reduction:.1}%",
            st.compressor_runs, st.tokens_raw_compressor
        ),
    );
    ui::print_kv(
        "interventions:",
        &format!(
            "stop_blocks={} security_prompts={} rule_hits={}",
            st.stop_blocks,
            st.security_prompts,
            st.rule_hits.values().sum::<u64>()
        ),
    );
}

fn print_top_rule_hits(st: &AnvilStats) {
    let mut rules: Vec<(&String, &u64)> = st.rule_hits.iter().collect();
    rules.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    if rules.is_empty() {
        return;
    }
    let max_hits = rules.first().map_or(1, |(_, c)| **c).max(1);
    ui::print_section("Top Rule Hits");
    if !ui::colors_enabled() {
        println!("  top_rule_hits:");
    }
    for (rule, count) in rules.into_iter().take(10) {
        let bar = ui::bar(*count, max_hits, 16);
        println!("    {:<24} {:>4}{bar}", ui::cyan(rule), count);
    }
}

fn print_trajectory_section(root: &Path, st: &AnvilStats) {
    let p50 = percentile_ms(&st.hook_latencies_ms, 50.0);
    let p95 = percentile_ms(&st.hook_latencies_ms, 95.0);
    ui::print_section("Trajectory & Latency");
    ui::print_kv(
        "latency:",
        &format!(
            "p50={}ms p95={}ms (samples={})",
            p50,
            p95,
            st.hook_latencies_ms.len()
        ),
    );
    ui::print_kv(
        "code_mode:",
        &format!(
            "calls={} tokens_saved={}",
            st.code_mode_calls, st.tokens_saved_code_mode
        ),
    );
    if let Ok(cfg) = AnvilConfig::load_or_default(root)
        && let Ok(cb) = CircuitBreaker::new(root, cfg.circuit_breaker_limit).status()
    {
        ui::print_kv("circuit:", &format!("{:?} (streak={})", cb.state, cb.consecutive_failures));
    }
}
