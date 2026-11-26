//! Integration tests for PHP 8.x & Laravel (API, Blade, Inertia, Livewire) AST extraction & Tier 0 rules.

use std::path::Path;

use anvil_ast::{analyze_file, detect_language};
use anvil_core::config::AnvilConfig;
use anvil_core::traits::{EdgeKind, Language, Severity, SymbolKind};

fn default_cfg() -> AnvilConfig {
    AnvilConfig::default()
}

#[test]
fn detects_php_and_blade_languages() {
    assert_eq!(detect_language(Path::new("app/Models/User.php")), Language::Php);
    assert_eq!(
        detect_language(Path::new("resources/views/users/index.blade.php")),
        Language::Blade
    );
}

const AUTH_CONTROLLER_PHP: &str = r#"<?php

declare(strict_types=1);

namespace App\Http\Controllers\Api;

use App\Http\Controllers\Controller;
use App\Http\Requests\Auth\{LoginRequest, ChangePasswordRequest};
use App\Services\Auth\LoginService;
use App\Support\Auth\RefreshCookie;

/**
 * Handles JWT authentication endpoints.
 */
#[CustomAttribute]
final class AuthController extends Controller implements Authenticatable
{
    use LogsActivity;

    public const VERSION = 'v2';

    public function __construct(private readonly LoginService $logins) {}

    public function login(LoginRequest $request): JsonResponse
    {
        $cookie = RefreshCookie::read($request);
        $dto = new SessionDto($cookie);
        return response()->json($dto);
    }

    private function helper(): void {}
}
"#;

#[test]
fn extracts_php8_symbols_imports_traits_di_and_calls() {
    let src = AUTH_CONTROLLER_PHP;

    let facts =
        analyze_file(Path::new("app/Http/Controllers/Api/AuthController.php"), src, &default_cfg())
            .expect("analyze php controller");
    assert!(facts.diagnostics.is_empty(), "unexpected diags: {:?}", facts.diagnostics);

    let sym_names: Vec<(&str, SymbolKind, bool)> =
        facts.symbols.iter().map(|s| (s.qualified_name.as_str(), s.kind, s.exported)).collect();
    for expected in [
        ("AuthController", SymbolKind::Class, true),
        ("AuthController::VERSION", SymbolKind::Constant, true),
        ("AuthController::__construct", SymbolKind::Method, true),
        ("AuthController::login", SymbolKind::Method, true),
        ("AuthController::helper", SymbolKind::Method, false),
    ] {
        assert!(sym_names.contains(&expected), "missing {expected:?} in {sym_names:?}");
    }

    let login_sym =
        facts.symbols.iter().find(|s| s.qualified_name == "AuthController::login").expect("login");
    assert!(login_sym.end_line > login_sym.line);

    for expected_edge in [
        (EdgeKind::Imports, "App\\Http\\Controllers\\Controller"),
        (EdgeKind::Imports, "App\\Http\\Requests\\Auth\\LoginRequest"),
        (EdgeKind::Imports, "App\\Http\\Requests\\Auth\\ChangePasswordRequest"),
        (EdgeKind::Imports, "App\\Services\\Auth\\LoginService"),
        (EdgeKind::Extends, "Controller"),
        (EdgeKind::Implements, "Authenticatable"),
        (EdgeKind::Implements, "LogsActivity"),
        (EdgeKind::Injects, "LoginService"),
        (EdgeKind::Injects, "LoginRequest"),
        (EdgeKind::Calls, "RefreshCookie::read"),
        (EdgeKind::Calls, "SessionDto"),
    ] {
        assert!(
            facts
                .edges
                .iter()
                .any(|e| e.kind == expected_edge.0 && e.target_path_or_symbol == expected_edge.1),
            "missing edge {expected_edge:?}"
        );
    }
}

#[test]
fn extracts_laravel_nested_routes_and_controllers() {
    let routes_src = r#"<?php

declare(strict_types=1);

use App\Http\Controllers\Api\ChargeController;
use Illuminate\Support\Facades\Route;

Route::middleware('jwt')->group(function (): void {
    Route::prefix('charges')->group(function (): void {
        Route::get('/', [ChargeController::class, 'index']);
        Route::get('{charge}', [ChargeController::class, 'show'])->whereNumber('charge');
        Route::post('{charge}/void', [ChargeController::class, 'void']);
    });
    Route::apiResource('invoices', ChargeController::class);
    Route::inertia('dashboard', 'Dashboard/Home');
});
"#;

    let facts =
        analyze_file(Path::new("backend/routes/api/billing.php"), routes_src, &default_cfg())
            .expect("analyze laravel routes");
    assert!(facts.diagnostics.is_empty(), "unexpected diags: {:?}", facts.diagnostics);

    let routes: Vec<&str> = facts
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::ExposesRoute)
        .map(|e| e.target_path_or_symbol.as_str())
        .collect();
    for expected in [
        "route:GET /api/charges",
        "route:GET /api/charges/:param",
        "route:POST /api/charges/:param/void",
        "route:GET /api/invoices",
        "route:DELETE /api/invoices/:param",
        "route:GET /api/dashboard",
    ] {
        assert!(routes.contains(&expected), "missing {expected} in {routes:?}");
    }
    assert!(
        facts
            .edges
            .iter()
            .any(|e| e.kind == EdgeKind::Calls
                && e.target_path_or_symbol == "ChargeController::void")
    );
    assert!(facts.edges.iter().any(|e| e.kind == EdgeKind::RendersTemplate
        && e.target_path_or_symbol == "inertia:Dashboard/Home"));
}

#[test]
fn extracts_laravel_views_inertia_and_container_bindings() {
    let ctrl_src = r#"<?php

declare(strict_types=1);

use Inertia\Inertia;

final class PageController
{
    public function show() { return view('admin.users.index'); }
    public function spa() { return Inertia::render('Users/Index'); }
    public function register(): void
    {
        $this->app->bind(UserRepo::class, EloquentUserRepo::class);
    }
}
"#;
    let facts = analyze_file(
        Path::new("app/Http/Controllers/PageController.php"),
        ctrl_src,
        &default_cfg(),
    )
    .expect("analyze page controller");
    assert!(facts.edges.iter().any(|e| e.kind == EdgeKind::RendersTemplate
        && e.target_path_or_symbol == "blade:admin.users.index"));
    assert!(
        facts.edges.iter().any(|e| e.kind == EdgeKind::RendersTemplate
            && e.target_path_or_symbol == "inertia:Users/Index")
    );
    assert!(
        facts
            .edges
            .iter()
            .any(|e| e.kind == EdgeKind::Provides && e.target_path_or_symbol == "EloquentUserRepo")
    );
}

#[test]
fn extracts_blade_directives_and_components() {
    let blade_src = r#"@extends('layouts.app')

@section('content')
    @include('partials.nav')
    <x-ui.button label="Save" />
    <livewire:billing.charge-table />
    @livewire('notifications-bell')
@endsection
"#;
    let facts = analyze_file(
        Path::new("resources/views/billing/index.blade.php"),
        blade_src,
        &default_cfg(),
    )
    .expect("analyze blade");
    assert!(
        facts.diagnostics.is_empty(),
        "blade should not require strict_types: {:?}",
        facts.diagnostics
    );

    let templates: Vec<&str> = facts
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::RendersTemplate)
        .map(|e| e.target_path_or_symbol.as_str())
        .collect();
    assert!(templates.contains(&"blade:layouts.app"));
    assert!(templates.contains(&"blade:partials.nav"));
    assert!(templates.contains(&"blade_component:ui.button"));
    assert!(templates.contains(&"livewire:billing.charge-table"));
    assert!(templates.contains(&"livewire:notifications-bell"));
}

#[test]
fn tier0_enforces_strict_types_and_escape_hatches() {
    let cfg = default_cfg();

    let missing_strict = "<?php\nfinal class Foo {}\n";
    let res = analyze_file(Path::new("app/Foo.php"), missing_strict, &cfg).expect("missing strict");
    assert!(
        res.diagnostics
            .iter()
            .any(|d| d.rule == "require-strict-types" && d.severity == Severity::Error)
    );

    let allowed_strict = "<?php\n// anvil-allow(require-strict-types): legacy framework entrypoint [PLAT-1]\nfinal class Foo {}\n";
    let ok =
        analyze_file(Path::new("public/index.php"), allowed_strict, &cfg).expect("allowed strict");
    assert!(ok.diagnostics.is_empty(), "unexpected: {:?}", ok.diagnostics);

    let escapes = "<?php\n\ndeclare(strict_types=1);\n\nfinal class Bad {\n  public function run(): void {\n    // @phpstan-ignore-next-line\n    $a = 1;\n    /** @psalm-suppress MixedAssignment */\n    $b = $a;\n    // phpcs:ignore Generic.Files.LineLength\n    $c = @file_get_contents('x');\n  }\n}\n";
    let esc_res = analyze_file(Path::new("app/Bad.php"), escapes, &cfg).expect("escapes");
    assert_eq!(esc_res.diagnostics.len(), 4, "diags: {:?}", esc_res.diagnostics);
    assert!(esc_res.diagnostics.iter().all(|d| d.rule == "no-escape-hatches"));
}

#[test]
fn tier0_enforces_php_debug_helpers_and_env_outside_config() {
    let cfg = default_cfg();
    let hygiene = "<?php\n\ndeclare(strict_types=1);\n\nfinal class Service {\n  public function debug(): void {\n    dd('boom');\n    $key = env('APP_KEY');\n  }\n}\n";
    let hyg_res =
        analyze_file(Path::new("app/Services/Service.php"), hygiene, &cfg).expect("hygiene");
    assert!(hyg_res.diagnostics.iter().any(|d| d.rule == "no-debug-helpers"));
    assert!(hyg_res.diagnostics.iter().any(|d| d.rule == "no-env-outside-config"));

    let config_php =
        "<?php\n\ndeclare(strict_types=1);\n\nreturn ['name' => env('APP_NAME', 'Laravel')];\n";
    let cfg_ok =
        analyze_file(Path::new("backend/config/app.php"), config_php, &cfg).expect("config env");
    assert!(cfg_ok.diagnostics.is_empty(), "unexpected: {:?}", cfg_ok.diagnostics);
}
