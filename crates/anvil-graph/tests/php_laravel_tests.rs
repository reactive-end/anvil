//! Integration tests for PHP PSR-4, Laravel routes/Blade/Inertia, and cross-stack blast radius.

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
    fs::write(path, content).expect("write fixture");
}

fn build_laravel_monorepo_fixture() -> TempDir {
    let tmp = TempDir::new().expect("tempdir");
    let root = tmp.path();

    write_repo_file(
        root,
        "backend/composer.json",
        r#"{
          "autoload": { "psr-4": { "App\\": "app/" } },
          "autoload-dev": { "psr-4": { "Tests\\": "tests/" } }
        }"#,
    );
    write_repo_file(
        root,
        "backend/vendor/ignored/Fake.php",
        "<?php\ndeclare(strict_types=1);\nclass Fake {}\n",
    );
    write_repo_file(
        root,
        "backend/app/Services/ChargeService.php",
        "<?php\n\ndeclare(strict_types=1);\n\nnamespace App\\Services;\n\nfinal class ChargeService\n{\n    public function listAll(): array { return []; }\n}\n",
    );
    write_repo_file(
        root,
        "backend/app/Http/Controllers/Api/ChargeController.php",
        "<?php\n\ndeclare(strict_types=1);\n\nnamespace App\\Http\\Controllers\\Api;\n\nuse App\\Services\\ChargeService;\nuse Inertia\\Inertia;\n\nfinal class ChargeController\n{\n    public function index(ChargeService $charges)\n    {\n        $items = $charges->listAll();\n        return Inertia::render('Billing/Index', ['items' => $items]);\n    }\n\n    public function printView() { return view('billing.receipt'); }\n}\n",
    );
    write_repo_file(
        root,
        "backend/routes/api/billing.php",
        "<?php\n\ndeclare(strict_types=1);\n\nuse App\\Http\\Controllers\\Api\\ChargeController;\nuse Illuminate\\Support\\Facades\\Route;\n\nRoute::prefix('charges')->group(function (): void {\n    Route::get('/', [ChargeController::class, 'index']);\n});\n",
    );
    write_repo_file(root, "backend/resources/views/layouts/app.blade.php", "<html>@yield('content')</html>\n");
    write_repo_file(root, "backend/resources/views/components/badge.blade.php", "<span>{{ $slot }}</span>\n");
    write_repo_file(root, "backend/resources/views/billing/receipt.blade.php", "@extends('layouts.app')\n<x-badge>Paid</x-badge>\n");
    write_repo_file(
        root,
        "backend/tests/Feature/Billing/ChargeControllerTest.php",
        "<?php\n\ndeclare(strict_types=1);\n\nnamespace Tests\\Feature\\Billing;\n\nuse App\\Http\\Controllers\\Api\\ChargeController;\n\nfinal class ChargeControllerTest\n{\n    public function test_index(): void { $ctrl = new ChargeController(); }\n}\n",
    );
    write_repo_file(root, "frontend/src/pages/Billing/Index.tsx", "export function BillingIndex() { return <div />; }\n");
    write_repo_file(root, "frontend/src/api/charges.ts", "export async function fetchCharges() { return fetch('/api/charges'); }\n");
    tmp
}

#[test]
fn resolves_psr4_and_cross_stack_route_blast_radius() {
    let tmp = build_laravel_monorepo_fixture();
    let engine = CodeGraphEngine::build(tmp.path(), AnvilConfig::default()).expect("build graph");
    assert!(!engine.graph.file_hashes.contains_key("backend/vendor/ignored/Fake.php"));

    let blast = engine.blast_radius(&["backend/app/Services/ChargeService.php".to_owned()], 3);
    let impacted: Vec<&str> = blast.impacted_files.iter().map(|i| i.file.as_str()).collect();
    assert!(impacted.contains(&"backend/app/Http/Controllers/Api/ChargeController.php"));
    assert!(impacted.contains(&"backend/routes/api/billing.php"));
    assert!(blast.related_tests.contains(&"backend/tests/Feature/Billing/ChargeControllerTest.php".to_owned()));

    let route_blast = engine.blast_radius(&["backend/routes/api/billing.php".to_owned()], 3);
    let route_impacted: Vec<&str> = route_blast.impacted_files.iter().map(|i| i.file.as_str()).collect();
    assert!(route_impacted.contains(&"frontend/src/api/charges.ts"));
}

#[test]
fn resolves_blade_inertia_and_php_read_symbol() {
    let tmp = build_laravel_monorepo_fixture();
    let engine = CodeGraphEngine::build(tmp.path(), AnvilConfig::default()).expect("build graph");

    let blade_blast = engine.blast_radius(&["backend/resources/views/components/badge.blade.php".to_owned()], 3);
    let blade_impacted: Vec<&str> = blade_blast.impacted_files.iter().map(|i| i.file.as_str()).collect();
    assert!(blade_impacted.contains(&"backend/resources/views/billing/receipt.blade.php"));
    assert!(blade_impacted.contains(&"backend/app/Http/Controllers/Api/ChargeController.php"));

    let inertia_blast = engine.blast_radius(&["frontend/src/pages/Billing/Index.tsx".to_owned()], 2);
    let inertia_impacted: Vec<&str> = inertia_blast.impacted_files.iter().map(|i| i.file.as_str()).collect();
    assert!(inertia_impacted.contains(&"backend/app/Http/Controllers/Api/ChargeController.php"));

    let slice = engine
        .read_symbol("backend/app/Http/Controllers/Api/ChargeController.php::ChargeController::index", true)
        .expect("read_symbol ok")
        .expect("method found");
    assert!(slice.body.contains("Inertia::render"));
    assert!(slice.imports.iter().any(|l| l.contains("ChargeService")));
}
