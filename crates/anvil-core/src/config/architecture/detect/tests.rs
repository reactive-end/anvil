use std::fs;
use std::path::Path;

use tempfile::TempDir;

use super::*;

fn dirs(root: &Path, paths: &[&str]) {
    for path in paths {
        fs::create_dir_all(root.join(path)).expect("mkdir");
    }
}

fn detect(paths: &[&str]) -> Suggestion {
    let dir = TempDir::new().expect("tempdir");
    dirs(dir.path(), paths);
    detect_style(dir.path())
}

#[test]
fn hexagonal_modules_with_domain_application_and_infrastructure() {
    let found = detect(&[
        "orders/domain",
        "orders/application",
        "orders/infraestructure",
        "orders/controller",
        "customers/domain",
        "customers/application",
        "customers/infrastructure",
        "shared/config",
    ]);
    assert_eq!(found.style, ArchitectureStyle::Hexagonal);
    assert_eq!(found.confidence, Confidence::High);
    assert!(found.note.contains("2 of 3 modules"), "{}", found.note);
}

#[test]
fn a_single_hexagonal_module_is_a_medium_confidence_hint() {
    let found = detect(&["orders/domain", "orders/application", "orders/infra"]);
    assert_eq!(found.style, ArchitectureStyle::Hexagonal);
    assert_eq!(found.confidence, Confidence::Medium);
}

#[test]
fn clean_architecture_from_entities_and_use_cases() {
    let found = detect(&["entities", "use-cases", "adapters", "frameworks"]);
    assert_eq!(found.style, ArchitectureStyle::Clean);
    assert_eq!(found.confidence, Confidence::High);
}

#[test]
fn feature_sliced_from_features_entities_and_shared() {
    let found = detect(&["app", "pages", "widgets", "features", "entities", "shared"]);
    assert_eq!(found.style, ArchitectureStyle::FeatureSliced);
    assert_eq!(found.confidence, Confidence::High);
}

#[test]
fn layered_from_controllers_services_and_repositories() {
    let found = detect(&["controllers", "services", "repositories"]);
    assert_eq!(found.style, ArchitectureStyle::Layered);
}

#[test]
fn unrecognized_layout_is_none_and_missing_root_is_low_confidence() {
    let found = detect(&["utils", "helpers", "components"]);
    assert_eq!(found.style, ArchitectureStyle::None);
    assert_eq!(found.confidence, Confidence::Low);

    let missing = detect_style(Path::new("definitely/not/here"));
    assert_eq!(missing.style, ArchitectureStyle::None);
    assert!(missing.note.contains("does not exist"));
}

#[test]
fn modules_folder_is_unwrapped_before_counting() {
    let found = detect(&[
        "modules/billing/domain",
        "modules/billing/application",
        "modules/billing/infrastructure",
        "modules/users/domain",
        "modules/users/application",
        "modules/users/persistence",
    ]);
    assert_eq!(found.style, ArchitectureStyle::Hexagonal);
    assert!(found.note.contains("2 of 2 modules"), "{}", found.note);
}

#[test]
fn a_ui_stack_without_a_server_framework_is_a_client_app() {
    assert_eq!(client_app_stack(&[StackKind::Vite, StackKind::TypeScript]), Some(StackKind::Vite));
    assert_eq!(client_app_stack(&[StackKind::React]), Some(StackKind::React));
    assert_eq!(
        client_app_stack(&[StackKind::Angular, StackKind::TypeScript]),
        Some(StackKind::Angular)
    );
}

#[test]
fn a_server_framework_or_no_ui_stack_is_not_a_client_app() {
    assert_eq!(client_app_stack(&[StackKind::NestJs, StackKind::TypeScript]), None);
    assert_eq!(client_app_stack(&[StackKind::Vite, StackKind::NestJs]), None);
    assert_eq!(client_app_stack(&[StackKind::NextJs, StackKind::React]), None);
    assert_eq!(client_app_stack(&[StackKind::TypeScript]), None);
    assert_eq!(client_app_stack(&[]), None);
}
