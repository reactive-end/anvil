//! Adds the configured architecture to the managed block of the agent instruction files.
//!
//! The templates stay generic; the `Architecture:` lines come from `.harness/config.json` so every
//! adapter (CLAUDE.md, AGENTS.md, Codex, Cursor, Windsurf) states the same style the checks enforce.

use anvil_core::config::AnvilConfig;
use anvil_core::config::architecture::guidance::architecture_lines;

use crate::sync::MANAGED_END;

/// Returns `template` with an `Architecture` section before the managed end marker, or `template`
/// unchanged when no style is configured.
pub fn with_architecture(template: &str, config: &AnvilConfig) -> String {
    let lines = architecture_lines(config);
    let Some(end) = template.rfind(MANAGED_END).filter(|_| !lines.is_empty()) else {
        return template.to_owned();
    };
    let bullets: String = lines.iter().map(|line| format!("- {line}\n")).collect();
    let before = template[..end].trim_end();
    let after = &template[end..];
    format!("{before}\n\n## Architecture\n\n{bullets}{after}")
}

#[cfg(test)]
mod tests {
    use anvil_core::config::architecture::ArchitectureStyle;

    use super::*;
    use crate::templates;

    fn configured(style: ArchitectureStyle) -> AnvilConfig {
        let mut config = AnvilConfig::default();
        config.set_architecture(style, None, None).expect("set");
        config
    }

    #[test]
    fn untouched_without_a_style() {
        let out = with_architecture(templates::CLAUDE_MD, &AnvilConfig::default());
        assert_eq!(out, templates::CLAUDE_MD);
    }

    #[test]
    fn section_lands_inside_the_managed_block_for_every_template() {
        let config = configured(ArchitectureStyle::Hexagonal);
        for template in [
            templates::CLAUDE_MD,
            templates::AGENTS_MD,
            templates::CODEX_AGENTS_MD,
            templates::CURSOR_RULES_MD,
            templates::WINDSURF_RULES_MD,
        ] {
            let out = with_architecture(template, &config);
            let section = out.find("## Architecture").expect("section");
            let end = out.find(MANAGED_END).expect("end marker");
            assert!(section < end, "section must be inside the managed block");
            assert!(
                out.contains("- Architecture: hexagonal (ports & adapters) in this repository:")
            );
            assert!(out.trim_end().ends_with(MANAGED_END));
        }
    }

    #[test]
    fn rendering_twice_is_stable() {
        let config = configured(ArchitectureStyle::Clean);
        let once = with_architecture(templates::AGENTS_MD, &config);
        assert_eq!(once, with_architecture(templates::AGENTS_MD, &config));
        assert_eq!(once.matches("## Architecture").count(), 1);
    }
}
