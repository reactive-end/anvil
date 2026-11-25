//! Compile-time embedded adapter and skill templates (`include_str!`).

/// Embedded `AGENTS.md` thin adapter template for Antigravity.
pub const AGENTS_MD: &str = include_str!("../../../templates/AGENTS.md");

/// Embedded `CLAUDE.md` thin adapter template for Claude Code.
pub const CLAUDE_MD: &str = include_str!("../../../templates/CLAUDE.md");

/// Embedded Cursor `.cursor/rules/anvil.mdc` body template.
pub const CURSOR_RULES_MD: &str = include_str!("../../../templates/CURSOR.md");

/// Embedded Windsurf `.windsurf/rules/anvil.md` template.
pub const WINDSURF_RULES_MD: &str = include_str!("../../../templates/WINDSURF.md");

/// Embedded `AGENTS.md` thin adapter template for standalone Codex repos.
pub const CODEX_AGENTS_MD: &str = include_str!("../../../templates/CODEX.md");

/// Embedded Claude Code agent that plans, decides, and reviews.
pub const ORCHESTRATOR_AGENT_MD: &str =
    include_str!("../../../templates/agents/anvil-orchestrator.md");

/// Embedded Claude Code agent that executes delegated steps.
pub const EXECUTOR_AGENT_MD: &str = include_str!("../../../templates/agents/anvil-executor.md");

/// Embedded `/orchestrate` skill for running the pair inside an existing session.
pub const ORCHESTRATE_SKILL_MD: &str =
    include_str!("../../../templates/agents/orchestrate-skill.md");

/// A built-in skill embedded at compile time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmbeddedSkill {
    pub name: &'static str,
    pub skill_md: &'static str,
}

/// All built-in `anvil` skills synced into `.harness/skills/`.
pub const EMBEDDED_SKILLS: &[EmbeddedSkill] = &[
    EmbeddedSkill {
        name: "subagent-routing",
        skill_md: include_str!("../../../templates/skills/subagent-routing/SKILL.md"),
    },
    EmbeddedSkill {
        name: "visual-qa",
        skill_md: include_str!("../../../templates/skills/visual-qa/SKILL.md"),
    },
    EmbeddedSkill {
        name: "ratchet-review",
        skill_md: include_str!("../../../templates/skills/ratchet-review/SKILL.md"),
    },
    EmbeddedSkill {
        name: "memory-hygiene",
        skill_md: include_str!("../../../templates/skills/memory-hygiene/SKILL.md"),
    },
];
