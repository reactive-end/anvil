use std::fs;

use anvil_core::config::{
    AnvilConfig, MobileQaDriver, StackKind, SubrepoConfig, ToolKind, WebQaDriver,
};
use anvil_qa::QaEngine;
use anvil_qa::model::UiPlatform;
use anvil_qa::onboarding::{
    ToolAvailability, build_onboarding_prompt, build_onboarding_prompt_with, detect_ui_platform,
    parse_mobile_driver, parse_web_driver,
};

#[test]
fn detects_web_mobile_both_and_none_platforms() {
    let web_cfg = AnvilConfig {
        stacks: vec![StackKind::React, StackKind::Vite],
        ..Default::default()
    };
    assert_eq!(detect_ui_platform(&web_cfg), UiPlatform::Web);

    let mobile_cfg = AnvilConfig {
        stacks: vec![StackKind::KotlinAndroid],
        ..Default::default()
    };
    assert_eq!(detect_ui_platform(&mobile_cfg), UiPlatform::Mobile);

    let multi_repo_cfg = AnvilConfig {
        subrepos: vec![
            SubrepoConfig {
                name: "fliiper-front".to_owned(),
                path: "fliiper-front".to_owned(),
                stacks: vec![StackKind::Angular],
                tools: vec![],
                sonar_enabled: false,
            },
            SubrepoConfig {
                name: "centinela-ia-movil".to_owned(),
                path: "centinela-ia-movil".to_owned(),
                stacks: vec![StackKind::KotlinAndroid],
                tools: vec![ToolKind::Roborazzi],
                sonar_enabled: false,
            },
        ],
        ..Default::default()
    };
    assert_eq!(detect_ui_platform(&multi_repo_cfg), UiPlatform::Both);

    let backend_cfg = AnvilConfig {
        stacks: vec![StackKind::NestJs, StackKind::Python],
        ..Default::default()
    };
    assert_eq!(detect_ui_platform(&backend_cfg), UiPlatform::None);
    assert!(
        build_onboarding_prompt_with(&backend_cfg, ToolAvailability::default()).is_none()
    );
}

#[test]
fn builds_roborazzi_recommendation_and_persists_drivers() {
    let dir = tempfile::tempdir().expect("create tempdir");
    fs::create_dir_all(dir.path().join("app")).expect("create app dir");
    fs::write(
        dir.path().join("app/build.gradle.kts"),
        "plugins { id(\"io.github.takahirom.roborazzi\") }\n",
    )
    .expect("write gradle");

    let cfg = AnvilConfig {
        stacks: vec![StackKind::Angular, StackKind::KotlinAndroid],
        tools: vec![ToolKind::Roborazzi],
        ..Default::default()
    };
    cfg.save(dir.path()).expect("save initial config");

    let engine = QaEngine::new(dir.path());
    assert_eq!(engine.detect_platform(&cfg), UiPlatform::Both);

    let prompt = build_onboarding_prompt(dir.path(), &cfg).expect("expected onboarding prompt");
    assert!(prompt.web_needed);
    assert!(prompt.mobile_needed);

    let roborazzi_opt = prompt
        .mobile_options
        .iter()
        .find(|opt| opt.id == "roborazzi")
        .expect("roborazzi option present");
    assert!(roborazzi_opt.available);
    assert!(roborazzi_opt.recommended);

    let web_driver = parse_web_driver("playwright").expect("parse web driver");
    let mobile_driver = parse_mobile_driver("roborazzi").expect("parse mobile driver");

    engine
        .select_web_driver(web_driver)
        .expect("persist web driver");
    let final_cfg = engine
        .select_mobile_driver(mobile_driver)
        .expect("persist mobile driver");

    let reloaded = AnvilConfig::load(dir.path()).expect("reload config");
    assert_eq!(reloaded, final_cfg);
    assert_eq!(reloaded.qa_driver.web, WebQaDriver::Playwright);
    assert_eq!(reloaded.qa_driver.mobile, MobileQaDriver::Roborazzi);
    assert!(engine.onboarding_prompt(&reloaded).is_none());
}
