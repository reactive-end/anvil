use super::*;
use crate::config::{AnvilConfig, BoundaryRule, SubrepoConfig};

fn config_with(style: ArchitectureStyle) -> AnvilConfig {
    let mut config = AnvilConfig::default();
    config.set_architecture(style, None, None).expect("set");
    config
}

fn rule<'a>(rules: &'a [ArchRule], name: &str) -> &'a ArchRule {
    rules.iter().find(|r| r.name == name).unwrap_or_else(|| {
        panic!("missing {name}; have {:?}", rules.iter().map(|r| &r.name).collect::<Vec<_>>())
    })
}

fn layer(rule: &ArchRule) -> (&[String], &[String]) {
    match &rule.kind {
        ArchRuleKind::Layer { from, forbid } => (from, forbid),
        other => panic!("not a layer rule: {other:?}"),
    }
}

#[test]
fn none_expands_to_nothing() {
    assert!(config_with(ArchitectureStyle::None).architecture_rules().is_empty());
}

#[test]
fn hexagonal_forbids_inward_imports_and_framework_packages() {
    let rules = config_with(ArchitectureStyle::Hexagonal).architecture_rules();
    assert!(rules.iter().all(|r| r.name.starts_with("arch:hexagonal:")));
    assert!(rules.iter().all(|r| !r.reason.is_empty()));

    let (from, forbid) = layer(rule(&rules, "arch:hexagonal:domain-no-infrastructure"));
    assert_eq!(from, ["src/**/domain/**"]);
    for folder in ["infrastructure", "infraestructure", "infra", "adapters", "persistence"] {
        assert!(forbid.contains(&format!("src/**/{folder}/**")), "{folder}");
    }
    let (_, delivery) = layer(rule(&rules, "arch:hexagonal:domain-no-delivery"));
    for folder in ["controller", "controllers", "presentation", "api", "http", "web", "graphql"] {
        assert!(delivery.contains(&format!("src/**/{folder}/**")), "{folder}");
    }
    let (from, forbid) = layer(rule(&rules, "arch:hexagonal:infrastructure-no-delivery"));
    assert!(from.contains(&"src/**/infra/**".to_owned()));
    assert!(forbid.contains(&"src/**/controllers/**".to_owned()));
}

#[test]
fn hexagonal_package_lists_keep_nestjs_common_allowed_in_application() {
    let rules = config_with(ArchitectureStyle::Hexagonal).architecture_rules();
    let ArchRuleKind::Packages { packages: domain, .. } =
        &rule(&rules, "arch:hexagonal:domain-no-framework").kind
    else {
        panic!("domain-no-framework must be a packages rule");
    };
    for pkg in ["@nestjs/*", "typeorm", "@prisma/*", "class-validator", "ioredis", "pg", "mysql2"] {
        assert!(domain.iter().any(|p| p == pkg), "domain must forbid {pkg}");
    }
    let ArchRuleKind::Packages { packages: app, .. } =
        &rule(&rules, "arch:hexagonal:application-no-orm-http").kind
    else {
        panic!("application-no-orm-http must be a packages rule");
    };
    assert!(app.iter().any(|p| p == "typeorm"));
    assert!(!app.iter().any(|p| p.starts_with("@nestjs")), "{app:?}");
}

#[test]
fn strict_framework_also_forbids_nestjs_in_application() {
    let mut config = config_with(ArchitectureStyle::Hexagonal);
    config.architecture.repo.strict_framework = Some(true);
    let rules = config.architecture_rules();
    let ArchRuleKind::Packages { packages, .. } =
        &rule(&rules, "arch:hexagonal:application-no-orm-http").kind
    else {
        panic!("packages rule expected");
    };
    assert!(packages.iter().any(|p| p == "@nestjs/*"));
}

#[test]
fn hexagonal_module_boundaries_only_expose_domain_application_and_module_files() {
    let mut config = config_with(ArchitectureStyle::Hexagonal);
    config.architecture.repo.modules = vec!["orders".to_owned(), "customers".to_owned()];
    let rules = config.architecture_rules();
    let ArchRuleKind::Modules(iso) = &rule(&rules, "arch:hexagonal:module-boundaries").kind else {
        panic!("module-boundaries must be a module isolation rule");
    };
    assert_eq!(iso.roots, ["src/modules", "src"]);
    assert_eq!(iso.public, ["domain/**", "application/**", "*.module.*", "index.*"]);
    assert_eq!(iso.only, ["orders", "customers"]);
    assert!(iso.exempt.contains(&"shared".to_owned()));
}

#[test]
fn clean_rings_never_import_outward() {
    let rules = config_with(ArchitectureStyle::Clean).architecture_rules();
    assert_eq!(rules.len(), 3);
    let (from, forbid) = layer(rule(&rules, "arch:clean:entities-inward-only"));
    assert!(
        from.contains(&"src/**/entities/**".to_owned())
            && from.contains(&"src/**/domain/**".to_owned())
    );
    for outer in
        ["use-cases", "application", "controllers", "gateways", "infrastructure", "drivers"]
    {
        assert!(forbid.contains(&format!("src/**/{outer}/**")), "{outer}");
    }
    let (_, forbid) = layer(rule(&rules, "arch:clean:adapters-inward-only"));
    assert!(forbid.contains(&"src/**/frameworks/**".to_owned()));
    assert!(!forbid.contains(&"src/**/use-cases/**".to_owned()));
}

#[test]
fn layered_keeps_repositories_below_services_below_controllers() {
    let rules = config_with(ArchitectureStyle::Layered).architecture_rules();
    let (from, forbid) = layer(rule(&rules, "arch:layered:data-no-upward"));
    assert!(
        from.contains(&"src/**/repositories/**".to_owned())
            && from.contains(&"src/**/dao/**".to_owned())
    );
    assert!(
        forbid.contains(&"src/**/services/**".to_owned())
            && forbid.contains(&"src/**/routes/**".to_owned())
    );
    let (from, forbid) = layer(rule(&rules, "arch:layered:services-no-controllers"));
    assert_eq!(from, ["src/**/service/**", "src/**/services/**"]);
    assert!(!forbid.iter().any(|g| g.contains("service")));
}

#[test]
fn feature_sliced_orders_layers_and_isolates_slices() {
    let rules = config_with(ArchitectureStyle::FeatureSliced).architecture_rules();
    let (from, forbid) = layer(rule(&rules, "arch:feature-sliced:features-imports-downward"));
    assert_eq!(from, ["src/features/**"]);
    assert_eq!(forbid, ["src/app/**", "src/processes/**", "src/pages/**", "src/widgets/**"]);
    assert_eq!(layer(rule(&rules, "arch:feature-sliced:shared-imports-downward")).1.len(), 6);
    assert!(rules.iter().all(|r| r.name != "arch:feature-sliced:app-imports-downward"));
    let ArchRuleKind::Modules(iso) =
        &rule(&rules, "arch:feature-sliced:features-slices-isolated").kind
    else {
        panic!("slice isolation expected");
    };
    assert_eq!(iso.roots, ["src/features"]);
    assert!(iso.public.is_empty(), "slices of a layer expose nothing to each other");
}

#[test]
fn modular_monolith_exposes_only_the_public_api() {
    let rules = config_with(ArchitectureStyle::ModularMonolith).architecture_rules();
    assert_eq!(rules.len(), 1);
    let ArchRuleKind::Modules(iso) = &rules[0].kind else { panic!("module isolation expected") };
    assert_eq!(rules[0].name, "arch:modular-monolith:module-public-api");
    assert_eq!(iso.public, ["index.*", "*.module.*", "public/**", "api/**"]);
    assert!(iso.from.is_empty());
}

#[test]
fn event_driven_isolates_handlers_but_lets_events_cross_contexts() {
    let rules = config_with(ArchitectureStyle::EventDriven).architecture_rules();
    assert_eq!(rules.len(), 1);
    let ArchRuleKind::Modules(iso) = &rules[0].kind else { panic!("module isolation expected") };
    assert_eq!(rules[0].name, "arch:event-driven:handler-context-isolation");
    assert_eq!(iso.from, ["src/**/handlers/**", "src/**/consumers/**", "src/**/subscribers/**"]);
    assert!(iso.public.iter().any(|p| p == "events/**"));
}

#[test]
fn subrepo_rules_are_prefixed_with_the_path_and_suffixed_with_the_name() {
    let mut config = AnvilConfig {
        subrepos: vec![SubrepoConfig {
            name: "orders-api".to_owned(),
            path: "services/orders-api".to_owned(),
            stacks: Vec::new(),
            tools: Vec::new(),
            sonar_enabled: false,
        }],
        ..AnvilConfig::default()
    };
    config.set_architecture(ArchitectureStyle::Hexagonal, Some("orders-api"), None).expect("set");
    config.set_architecture(ArchitectureStyle::Clean, None, Some("app")).expect("root");

    let rules = config.architecture_rules();

    let (from, _) = layer(rule(&rules, "arch:hexagonal:domain-no-application@orders-api"));
    assert_eq!(from, ["services/orders-api/src/**/domain/**"]);
    let (from, _) = layer(rule(&rules, "arch:clean:entities-inward-only"));
    assert!(from.iter().all(|g| g.starts_with("app/")), "{from:?}");
}

#[test]
fn generated_rules_never_touch_the_users_boundaries() {
    let user = BoundaryRule {
        name: "mine".to_owned(),
        from_glob: "src/a/**".to_owned(),
        forbid_glob: "src/b/**".to_owned(),
        reason: Some("because".to_owned()),
    };
    let mut config = config_with(ArchitectureStyle::Hexagonal);
    config.boundaries = vec![user.clone()];

    let rules = config.architecture_rules();

    assert!(!rules.is_empty());
    assert_eq!(config.boundaries, vec![user]);
    let saved = serde_json::to_string(&config).expect("serialize");
    assert!(!saved.contains("arch:hexagonal"), "generated rules must not be persisted: {saved}");
}
