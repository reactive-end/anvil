//! Integration tests for React/TS prop-breakage, Angular selector blast-radius, and overengineering.

use std::fs;
use std::path::Path;

use anvil_core::config::AnvilConfig;
use anvil_graph::CodeGraphEngine;
use tempfile::TempDir;

fn write_repo_file(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(path, content).expect("write fixture file");
}

#[test]
fn react_prop_breakage_blast_radius_excludes_unrelated_files() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    write_repo_file(
        root,
        "tsconfig.json",
        r#"{
          "compilerOptions": {
            "baseUrl": "./",
            "paths": { "@components/*": ["src/components/*"] }
          }
        }"#,
    );
    write_repo_file(
        root,
        "src/components/Button.tsx",
        "export interface ButtonProps { variant: 'primary' | 'ghost'; }\n\
         export function Button({ variant }: ButtonProps) { return <button>{variant}</button>; }\n",
    );
    write_repo_file(
        root,
        "src/screens/CheckoutScreen.tsx",
        "import { Button } from '@components/Button';\n\
         export function CheckoutScreen() { return <Button variant=\"primary\" />; }\n",
    );
    write_repo_file(
        root,
        "src/components/Button.test.tsx",
        "import { Button } from './Button';\n\
         export function testBtn() { return Button({ variant: 'primary' }); }\n",
    );
    write_repo_file(
        root,
        "src/components/UnrelatedCard.tsx",
        "export function UnrelatedCard() { return <div>Card</div>; }\n",
    );
    write_repo_file(
        root,
        "src/components/UnrelatedCard.test.tsx",
        "import { UnrelatedCard } from './UnrelatedCard';\n\
         export function testCard() { return UnrelatedCard(); }\n",
    );

    let engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build graph");
    let report = engine.blast_radius(&["src/components/Button.tsx".to_owned()], 2);

    let impacted_paths: Vec<&str> = report
        .impacted_files
        .iter()
        .map(|i| i.file.as_str())
        .collect();
    assert!(
        impacted_paths.contains(&"src/screens/CheckoutScreen.tsx"),
        "expected CheckoutScreen.tsx in impacted: {impacted_paths:?}"
    );
    assert!(
        !impacted_paths.contains(&"src/components/UnrelatedCard.tsx"),
        "unexpected UnrelatedCard.tsx in impacted: {impacted_paths:?}"
    );
    let checkout = report
        .impacted_files
        .iter()
        .find(|i| i.file == "src/screens/CheckoutScreen.tsx")
        .expect("checkout item");
    assert!(checkout.is_screen);

    assert_eq!(
        report.related_tests,
        vec!["src/components/Button.test.tsx".to_owned()]
    );
}

#[test]
fn angular_selector_modification_impacts_consuming_html_template() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    write_repo_file(
        root,
        "src/app/header/header.component.ts",
        "import { Component } from '@angular/core';\n\
         @Component({ selector: 'app-header', templateUrl: './header.component.html' })\n\
         export class HeaderComponent { title = 'Anvil'; }\n",
    );
    write_repo_file(
        root,
        "src/app/header/header.component.html",
        "<header>{{ title }}</header>\n",
    );
    write_repo_file(
        root,
        "src/app/app.component.html",
        "<main><app-header [title]=\"mainTitle\"></app-header></main>\n",
    );
    write_repo_file(
        root,
        "src/app/app.component.ts",
        "@Component({ selector: 'app-root', templateUrl: './app.component.html' })\n\
         export class AppComponent { mainTitle = 'Home'; }\n",
    );
    write_repo_file(
        root,
        "src/app/footer.component.ts",
        "@Component({ selector: 'app-footer' })\n\
         export class FooterComponent {}\n",
    );

    let engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build angular graph");
    let report = engine.blast_radius(&["src/app/header/header.component.ts".to_owned()], 2);
    let impacted: Vec<&str> = report
        .impacted_files
        .iter()
        .map(|i| i.file.as_str())
        .collect();

    assert!(
        impacted.contains(&"src/app/app.component.html"),
        "expected app.component.html impacted via <app-header>, got: {impacted:?}"
    );
    assert!(
        impacted.contains(&"src/app/app.component.ts"),
        "expected app.component.ts impacted transitively, got: {impacted:?}"
    );
    assert!(!impacted.contains(&"src/app/footer.component.ts"));
}

#[test]
fn overengineering_check_flags_unused_new_abstraction_on_small_fix() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    write_repo_file(
        root,
        "src/core/SingleUseFactory.ts",
        "export class SingleUseFactory { create() { return 1; } }\n",
    );
    write_repo_file(
        root,
        "src/app.ts",
        "import { SingleUseFactory } from './core/SingleUseFactory';\n\
         export const x = new SingleUseFactory().create();\n",
    );

    let engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build");
    let report = engine.check_overengineering(
        &["src/core/SingleUseFactory.ts".to_owned()],
        &["src/core/SingleUseFactory.ts".to_owned()],
        true,
    );
    assert!(report.flagged);
    assert!(!report.warnings.is_empty());
}
