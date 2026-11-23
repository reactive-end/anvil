//! `anvil stats`: inspect or reset local hook latency, cache hits, token savings, and rule hits.

use std::path::Path;

use anyhow::Result;
use anvil_core::stats::{load_stats, reset_stats};

use crate::ui;

/// Executes `anvil stats [--reset]`.
pub fn execute(root: &Path, reset: bool) -> Result<()> {
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
        .saturating_add(st.tokens_saved_outline);

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
    ui::print_kv(
        "tokens_saved:",
        &format!(
            "total={} (compressor={}, symbol_read={}, outline={})",
            ui::green(&total_tokens_saved.to_string()),
            st.tokens_saved_compressor,
            st.tokens_saved_symbol_read,
            st.tokens_saved_outline
        ),
    );
    ui::print_kv(
        "token_guard:",
        &format!(
            "blocks={} warnings={}",
            ui::yellow(&st.token_guard_blocks.to_string()),
            st.token_guard_warnings
        ),
    );

    let mut rules: Vec<(&String, &u64)> = st.rule_hits.iter().collect();
    rules.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    if !rules.is_empty() {
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
    Ok(())
}
