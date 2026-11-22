//! `anvil stats`: inspect or reset local hook latency, cache hits, token savings, and rule hits.

use std::path::Path;

use anyhow::Result;
use anvil_core::stats::{load_stats, reset_stats};

/// Executes `anvil stats [--reset]`.
pub fn execute(root: &Path, reset: bool) -> Result<()> {
    if reset {
        reset_stats(root)?;
        println!("stats reset");
        return Ok(());
    }

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

    println!("anvil stats:");
    println!(
        "  hooks: calls={} avg_ms={avg_ms:.2} total_ms={}",
        st.hook_calls, st.total_hook_ms
    );
    println!("  check_cache_hits: {}", st.check_cache_hits);
    println!(
        "  tokens_saved: total={total_tokens_saved} (compressor={}, symbol_read={}, outline={})",
        st.tokens_saved_compressor, st.tokens_saved_symbol_read, st.tokens_saved_outline
    );
    println!(
        "  token_guard: blocks={} warnings={}",
        st.token_guard_blocks, st.token_guard_warnings
    );

    let mut rules: Vec<(&String, &u64)> = st.rule_hits.iter().collect();
    rules.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    if !rules.is_empty() {
        println!("  top_rule_hits:");
        for (rule, count) in rules.into_iter().take(10) {
            println!("    {rule}: {count}");
        }
    }
    Ok(())
}
