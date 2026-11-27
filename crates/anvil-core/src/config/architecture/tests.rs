use super::*;
use crate::config::SubrepoConfig;

fn subrepo(name: &str, path: &str) -> SubrepoConfig {
    SubrepoConfig {
        name: name.to_owned(),
        path: path.to_owned(),
        stacks: Vec::new(),
        tools: Vec::new(),
        sonar_enabled: false,
    }
}

#[test]
fn styles_round_trip_through_names_and_aliases() {
    for style in ArchitectureStyle::ALL {
        assert_eq!(style.as_str().parse::<ArchitectureStyle>().expect("parse"), style);
    }
    assert_eq!(
        "FSD".parse::<ArchitectureStyle>().expect("alias"),
        ArchitectureStyle::FeatureSliced
    );
    assert_eq!(
        "ports_and_adapters".parse::<ArchitectureStyle>().expect("alias"),
        ArchitectureStyle::Hexagonal
    );
    assert!("spaghetti".parse::<ArchitectureStyle>().is_err());
}

#[test]
fn old_configs_without_architecture_still_load_and_stay_unchanged() {
    let config: AnvilConfig =
        serde_json::from_str(r#"{"version":1,"stacks":["nest_js"]}"#).expect("parse");
    assert!(config.architecture.is_default());
    assert!(config.architecture_rules().is_empty());
    let json = serde_json::to_string(&config).expect("serialize");
    assert!(!json.contains("architecture"), "default section must not be written: {json}");
}

#[test]
fn config_json_shape_matches_the_documented_section() {
    let raw = r#"{"architecture":{"style":"hexagonal","source_root":"app","modules":["orders"],
        "strict_framework":true,"subrepos":{"api":{"style":"clean"}}}}"#;
    let config: AnvilConfig = serde_json::from_str(raw).expect("parse");
    assert_eq!(config.architecture.repo.style, ArchitectureStyle::Hexagonal);
    assert_eq!(config.architecture.repo.source_root.as_deref(), Some("app"));
    assert_eq!(config.architecture.repo.modules, ["orders"]);
    assert_eq!(config.architecture.repo.strict_framework, Some(true));
    assert_eq!(config.architecture.subrepos["api"].style, ArchitectureStyle::Clean);
}

#[test]
fn source_dir_is_anchored_under_the_subrepo_path() {
    let setting = ArchitectureSetting::default();
    let root = ArchTarget { subrepo: None, path: ".", setting: &setting };
    let sub = ArchTarget { subrepo: Some("api"), path: "services/api", setting: &setting };
    assert_eq!(root.source_dir(), "src");
    assert_eq!(sub.source_dir(), "services/api/src");

    let custom = ArchitectureSetting { source_root: Some("./lib/".to_owned()), ..setting };
    let target = ArchTarget { subrepo: None, path: ".", setting: &custom };
    assert_eq!(target.source_dir(), "lib");
}

#[test]
fn set_architecture_validates_subrepos_and_survives_a_subrepo_rebuild() {
    let mut config = AnvilConfig {
        subrepos: vec![subrepo("api", "api"), subrepo("web", "web")],
        ..AnvilConfig::default()
    };
    let err =
        config.set_architecture(ArchitectureStyle::Clean, Some("nope"), None).expect_err("unknown");
    assert!(err.to_string().contains("api, web"), "{err}");

    config.set_architecture(ArchitectureStyle::Hexagonal, Some("api"), Some("lib")).expect("set");
    // Stack detection clears and rebuilds `subrepos`; the choice lives elsewhere and is kept.
    config.subrepos.clear();
    config.subrepos.push(subrepo("api", "api"));
    let targets = config.architecture_targets();
    assert_eq!(targets.len(), 1);
    assert_eq!(targets[0].subrepo, Some("api"));
    assert_eq!(targets[0].source_dir(), "api/lib");

    config.set_architecture(ArchitectureStyle::None, Some("api"), None).expect("clear");
    assert!(config.architecture.subrepos.is_empty());
}
