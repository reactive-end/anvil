//! Agent-facing statement of the configured architecture, generated from the config.

use super::ArchitectureStyle;
use super::REPO_LABEL;
use crate::config::AnvilConfig;

/// What an agent must do to respect `style`, as one sentence fragment (empty for `none`).
pub fn guidance(style: ArchitectureStyle) -> &'static str {
    match style {
        ArchitectureStyle::None => "",
        ArchitectureStyle::Hexagonal => {
            "domain imports no framework or other layer; use cases depend on ports; adapters live in infrastructure; wire them in *.module.ts."
        }
        ArchitectureStyle::Clean => {
            "dependencies point inward (entities <- use cases <- interface adapters <- frameworks); declare ports in the inner ring and implement them in the outer one."
        }
        ArchitectureStyle::Layered => {
            "controllers call services and services call repositories; never import upward (repository -> service -> controller)."
        }
        ArchitectureStyle::FeatureSliced => {
            "layers app > processes > pages > widgets > features > entities > shared import only downward; slices of one layer never import each other."
        }
        ArchitectureStyle::ModularMonolith => {
            "modules talk to each other only through their public API (index, *.module, public/, api/); never import another module's internals."
        }
        ArchitectureStyle::EventDriven => {
            "publish and consume through the event bus; handlers are idempotent; no direct calls into another context."
        }
    }
}

/// One `Architecture: ...` line per distinct configured style, naming every place it applies.
///
/// Targets sharing a style (a legacy root plus subrepo entries, or several subrepos) are merged into
/// one line, so the same rules are never stated twice. Lines follow the order each style first appears.
pub fn architecture_lines(config: &AnvilConfig) -> Vec<String> {
    let mut groups: Vec<(ArchitectureStyle, Vec<&str>)> = Vec::new();
    for target in config.architecture_targets() {
        let style = target.setting.style;
        let label = target.subrepo.unwrap_or(REPO_LABEL);
        match groups.iter_mut().find(|(s, _)| *s == style) {
            Some((_, labels)) => {
                if !labels.contains(&label) {
                    labels.push(label);
                }
            }
            None => groups.push((style, vec![label])),
        }
    }
    groups
        .iter()
        .map(|(style, labels)| {
            format!(
                "Architecture: {style} ({}) in {}: {} Checked by `anvil check`; see `anvil architecture show`.",
                style.label(),
                labels.join(", "),
                guidance(*style),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SubrepoConfig;

    fn subrepo(name: &str) -> SubrepoConfig {
        SubrepoConfig {
            name: name.to_owned(),
            path: name.to_owned(),
            stacks: Vec::new(),
            tools: Vec::new(),
            sonar_enabled: false,
        }
    }

    #[test]
    fn no_lines_without_a_configured_style() {
        assert!(architecture_lines(&AnvilConfig::default()).is_empty());
    }

    #[test]
    fn one_line_per_configured_style_names_where_it_applies() {
        let mut config =
            AnvilConfig { subrepos: vec![subrepo("orders-api")], ..AnvilConfig::default() };
        config.set_architecture(ArchitectureStyle::EventDriven, None, None).expect("root");
        config
            .set_architecture(ArchitectureStyle::Hexagonal, Some("orders-api"), None)
            .expect("sub");

        let lines = architecture_lines(&config);

        assert_eq!(lines.len(), 2);
        assert!(
            lines[0]
                .starts_with("Architecture: event-driven (events & handlers) in this repository:")
        );
        assert!(lines[0].contains("handlers are idempotent"));
        assert!(
            lines[1].starts_with("Architecture: hexagonal (ports & adapters) in orders-api: domain imports no framework"),
            "{}",
            lines[1]
        );
        assert!(lines[1].contains("wire them in *.module.ts"));
    }

    #[test]
    fn subrepos_sharing_a_style_get_one_line() {
        let mut config = AnvilConfig {
            subrepos: vec![subrepo("orders-api"), subrepo("billing-api")],
            ..AnvilConfig::default()
        };
        for name in ["orders-api", "billing-api"] {
            config.set_architecture(ArchitectureStyle::Hexagonal, Some(name), None).expect("sub");
        }

        let lines = architecture_lines(&config);

        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].starts_with("Architecture: hexagonal (ports & adapters) in orders-api, billing-api: domain imports"),
            "{}",
            lines[0]
        );
    }

    #[test]
    fn a_legacy_root_plus_subrepos_config_renders_each_style_once() {
        let raw = r#"{"subrepos":[
            {"name":"orders-api","path":"orders-api"},{"name":"orders-web","path":"orders-web"}],
            "architecture":{"style":"hexagonal","source_root":"orders-api/src","subrepos":{
            "orders-api":{"style":"hexagonal"},"orders-web":{"style":"hexagonal"}}}}"#;
        let config: AnvilConfig = serde_json::from_str(raw).expect("parse");

        let lines = architecture_lines(&config);

        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(
            lines[0].contains("in this repository, orders-api, orders-web: domain imports"),
            "{}",
            lines[0]
        );
    }
}
