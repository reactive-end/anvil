//! Which repository paths a search may read, shared by the built-in and `tgrep` backends.

/// Entries never searched, even with `--hidden`.
const SKIPPED_ENTRIES: [&str; 7] =
    [".git", ".tgrep", ".venv", "__pycache__", ".dart_tool", ".gradle", "coverage"];

/// Dot-entries searched even without `--hidden`: CI workflows, `.harness/`, and agent configuration.
const VISIBLE_DOT_ENTRIES: [&str; 4] = [".github", ".harness", ".claude", ".agents"];

/// Children of `.harness/` that agents should read; ledgers (`tasks`, `memory`, `candidates`) and runtime state are not.
const HARNESS_SEARCHABLE: [&str; 4] = ["docs", "rules", "skills", "config.json"];

/// Globs that keep `tgrep` (run with `--hidden`) away from paths this module always rejects.
pub(crate) const TGREP_EXCLUDE_GLOBS: [&str; 6] = [
    "!.git/",
    "!.tgrep/",
    "!/.harness/tasks/",
    "!/.harness/memory/",
    "!/.harness/candidates/",
    "!/.harness/.state/",
];

/// Decides whether the entry `name` at repo-relative path `rel` may be searched.
///
/// Only the entry's own name is checked for dot-files, so a walk rooted at a hidden directory
/// that was named explicitly still searches inside it.
pub(crate) fn entry_allowed(rel: &str, name: &str, include_hidden: bool) -> bool {
    if SKIPPED_ENTRIES.contains(&name) || !harness_child_allowed(rel) {
        return false;
    }
    include_hidden || !name.starts_with('.') || VISIBLE_DOT_ENTRIES.contains(&name)
}

/// Decides whether a file path as a whole may be searched: every component must pass [`entry_allowed`].
pub(crate) fn is_searchable_path(rel: &str, include_hidden: bool) -> bool {
    rel.split('/')
        .filter(|part| !part.is_empty())
        .all(|name| entry_allowed(rel, name, include_hidden))
}

fn harness_child_allowed(rel: &str) -> bool {
    match rel.strip_prefix(".harness/") {
        None => true,
        Some(rest) => {
            let first = rest.split('/').next().unwrap_or_default();
            HARNESS_SEARCHABLE.contains(&first)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_exposes_only_agent_readable_children() {
        for ok in [
            ".harness/docs/a.md",
            ".harness/rules/r.md",
            ".harness/skills/s/SKILL.md",
            ".harness/config.json",
        ] {
            assert!(is_searchable_path(ok, false), "{ok}");
        }
        for no in [
            ".harness/tasks/T-1/progress.md",
            ".harness/memory/chunks/c.json",
            ".harness/candidates/x.json",
            ".harness/.state/graph.bin",
            ".harness/.worktrees/w/a.rs",
            ".harness/mcp-proxy.json",
        ] {
            assert!(!is_searchable_path(no, false), "{no}");
            assert!(!is_searchable_path(no, true), "{no} even with --hidden");
        }
    }

    #[test]
    fn dot_entries_follow_the_hidden_flag() {
        assert!(is_searchable_path(".github/workflows/ci.yml", false));
        assert!(is_searchable_path(".claude/settings.json", false));
        assert!(!is_searchable_path(".secret/x.txt", false));
        assert!(is_searchable_path(".secret/x.txt", true));
        assert!(!is_searchable_path(".git/config", true));
        assert!(!is_searchable_path(".tgrep/index", true));
        assert!(is_searchable_path("src/app.rs", false));
    }
}
