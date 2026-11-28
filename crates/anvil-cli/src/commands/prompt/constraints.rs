//! The rules a request must respect, taken from this repository's own configuration.

use anvil_core::config::AnvilConfig;
use anvil_core::config::architecture::guidance::architecture_lines;

/// Escape hatches an agent must not use without a structured justification.
const ESCAPE_HATCHES: &str =
    "`@ts-ignore`, `eslint-disable`, `# noqa`, `#[allow]`, `!important`, TODO/FIXME";

/// Constraint lines for `config`: architecture, escape hatches, security, and conventions.
pub fn constraints(config: &AnvilConfig) -> Vec<String> {
    let mut lines = architecture_lines(config);
    let marker = config.allowed_escapes.first().map_or("anvil-allow", String::as_str);
    lines.push(format!(
        "Do not add suppressions or escape hatches ({ESCAPE_HATCHES}); if one is truly needed, justify it as `{marker}(<rule>): <reason>`."
    ));
    lines.push(
        "Destructive shell commands, secret access, and force pushes go through the security guard and need the user's confirmation."
            .to_owned(),
    );
    lines.extend(conventions(config));
    lines
}

fn conventions(config: &AnvilConfig) -> Vec<String> {
    let mut lines = Vec::new();
    if !config.stacks.is_empty() {
        let stacks: Vec<String> = config.stacks.iter().map(|s| format!("{s:?}")).collect();
        lines.push(format!("Stacks: {}.", stacks.join(", ")));
    }
    if !config.boundaries.is_empty() {
        lines.push(format!(
            "Respect the {} custom import boundary rule(s) in `.harness/config.json`.",
            config.boundaries.len()
        ));
    }
    if config.forbid_import_cycles {
        lines.push("Import cycles are forbidden.".to_owned());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use anvil_core::config::StackKind;
    use anvil_core::config::architecture::ArchitectureStyle;

    #[test]
    fn a_plain_config_still_states_the_escape_hatch_and_security_rules() {
        let lines = constraints(&AnvilConfig::default());
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[0].contains("`anvil-allow(<rule>): <reason>`"));
        assert!(lines[1].contains("security guard"));
    }

    #[test]
    fn architecture_and_stack_lines_come_from_the_config() {
        let mut config = AnvilConfig { stacks: vec![StackKind::NestJs], ..AnvilConfig::default() };
        config.forbid_import_cycles = true;
        config.set_architecture(ArchitectureStyle::Hexagonal, None, None).unwrap();
        let lines = constraints(&config);
        assert!(lines[0].starts_with("Architecture: hexagonal (ports & adapters)"), "{lines:?}");
        assert!(lines.iter().any(|l| l == "Stacks: NestJs."));
        assert!(lines.iter().any(|l| l == "Import cycles are forbidden."));
    }
}
