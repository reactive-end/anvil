//! `anvil grep`: noise-filtered, binary-safe, token-bounded repository search.

use std::path::Path;

use anyhow::Result;
use anvil_core::diff_grep::compressed_grep;

/// Executes `anvil grep <pattern> [--max-matches N] [--max-tokens N]`.
pub fn execute(root: &Path, pattern: &str, max_matches: usize, max_tokens: usize) -> Result<()> {
    let res = compressed_grep(root, pattern, max_matches, max_tokens)?;
    if !res.text.is_empty() {
        println!("{}", res.text);
    }
    println!(
        "grep: {}/{} match(es) across {} file(s) (truncated={}, tokens_saved={})",
        res.matches.len(),
        res.total_matches,
        res.files_searched,
        res.truncated,
        res.estimated_tokens_saved
    );
    Ok(())
}
