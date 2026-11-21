//! Compile-time embedded adapter and skill templates (`include_str!`).

/// Embedded `AGENTS.md` thin adapter template for Antigravity.
pub const AGENTS_MD: &str = include_str!("../../../templates/AGENTS.md");

/// Embedded `CLAUDE.md` thin adapter template for Claude Code.
pub const CLAUDE_MD: &str = include_str!("../../../templates/CLAUDE.md");

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
