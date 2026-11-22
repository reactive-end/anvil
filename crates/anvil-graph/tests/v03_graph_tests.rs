//! Integration tests for `anvil-graph` v0.3.0 deliverables:
//! `read_symbol`, `outline`, `end_line` + `VERSION = 3` persistence,
//! cross-repo HTTP route blast-radius, and `tsgo`-first off-repo typecheck command.

use std::fs;
use std::path::Path;

use anvil_core::config::{AnvilConfig, StackKind};
use anvil_graph::CodeGraphEngine;
use anvil_graph::impact::{build_ts_typecheck_command, compute_blast_radius};
use anvil_graph::merge::merge_graphs;
use anvil_graph::model::NodeKind;
use anvil_graph::store;
use tempfile::TempDir;

fn write_repo_file(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent dir");
    }
    fs::write(path, content).expect("write fixture file");
}

fn create_mock_bin(work_dir: &Path, name: &str) {
    let bin_dir = work_dir.join("node_modules").join(".bin");
    fs::create_dir_all(&bin_dir).expect("create .bin dir");
    let file_name = if cfg!(windows) {
        format!("{name}.cmd")
    } else {
        name.to_owned()
    };
    let path = bin_dir.join(file_name);
    fs::write(&path, "@echo off\r\nexit /b 0\r\n").expect("write mock bin");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path).expect("meta").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).expect("chmod");
    }
}

fn build_billing_fixture(root: &Path) {
    let mut src = String::from(
        "import { TaxRate } from './tax';\n\
         import { Currency } from './currency';\n\n\
         export function calculateTotal(amount: number, rate: TaxRate): number {\n\
           const tax = amount * rate.value;\n\
           return amount + tax;\n\
         }\n\n",
    );
    for idx in 0..35 {
        src.push_str(&format!(
            "export function unrelatedHelper{idx}(x: number): number {{\n  return x + {idx};\n}}\n\n"
        ));
    }
    write_repo_file(root, "src/billing.ts", &src);
}

#[test]
fn read_symbol_returns_exact_span_imports_and_positive_tokens_saved() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    build_billing_fixture(root);

    let engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build");

    let slice = engine
        .read_symbol("src/billing.ts::calculateTotal", true)
        .expect("read_symbol ok")
        .expect("symbol found");
    assert_eq!((slice.file.as_str(), slice.symbol.as_str(), slice.kind.as_str()), ("src/billing.ts", "calculateTotal", "function"));
    assert_eq!((slice.start_line, slice.end_line), (4, 7));
    assert!(slice.total_file_lines > 100 && slice.imports.len() == 2);
    assert!(slice.imports[0].contains("TaxRate"));
    assert!(slice.body.contains("const tax = amount * rate.value;"));
    assert!(!slice.body.contains("unrelatedHelper0"));
    assert!(slice.estimated_tokens_saved > 0);

    let no_imports = engine
        .read_symbol("calculateTotal", false)
        .expect("read_symbol ok")
        .expect("symbol found by label");
    assert!(no_imports.imports.is_empty());
    assert!(no_imports.estimated_tokens_saved >= slice.estimated_tokens_saved);

    let bin_path = engine.save().expect("save");
    let hdr = store::read_header(&bin_path).expect("read header");
    assert_eq!(hdr.version, 3);

    let reloaded = CodeGraphEngine::load_cached(root, AnvilConfig::default()).expect("load_cached");
    let reloaded_slice = reloaded
        .read_symbol("calculateTotal", false)
        .expect("read_symbol after reload")
        .expect("symbol after reload");
    assert_eq!((reloaded_slice.start_line, reloaded_slice.end_line), (4, 7));
}

#[test]
fn read_symbol_caps_huge_bodies_at_250_lines() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    let mut huge = String::from("export function hugePipeline(n: number): number {\n  let acc = n;\n");
    for i in 0..280 {
        huge.push_str(&format!("  acc += {i};\n"));
    }
    huge.push_str("  return acc;\n}\n");
    write_repo_file(root, "src/huge.ts", &huge);

    let engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build");
    let slice = engine
        .read_symbol("hugePipeline", false)
        .expect("read_symbol")
        .expect("found");
    assert!(slice.end_line > 280);
    assert!(slice.body.contains("truncated"));
    assert!(slice.body.lines().count() <= 252);
}

#[test]
fn outline_returns_compact_signatures_for_file_and_dir_within_budget() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    write_repo_file(
        root,
        "src/orders/orders.controller.ts",
        "import { Controller, Get } from '@nestjs/common';\n\
         @Controller('orders')\n\
         export class OrdersController {\n\
           @Get(':id')\n\
           findOne(id: string) {\n\
             const hiddenSecretBody = 'should_not_appear_in_outline';\n\
             return { id, hiddenSecretBody };\n\
           }\n\
         }\n",
    );
    write_repo_file(
        root,
        "src/orders/orders.internal.ts",
        "function helperOne(a: number) { return a + 1; }\n\
         function helperTwo(b: number) { return b + 2; }\n",
    );
    write_repo_file(
        root,
        "src/auth/auth.service.ts",
        "export function verifyJwt(token: string): boolean {\n\
           return token.length > 10;\n\
         }\n",
    );

    let engine = CodeGraphEngine::build(root, AnvilConfig::default()).expect("build");

    let single = engine.outline(Some("src/orders/orders.controller.ts"), 4000);
    assert_eq!(single.files_total, 1);
    assert_eq!(single.files.len(), 1);
    assert!(!single.truncated);
    assert!(single.estimated_tokens_saved > 0);
    assert!(single.files[0].routes.iter().any(|r| r.contains("GET /orders/:param")));
    let json_single = serde_json::to_string(&single).expect("serialize");
    assert!(!json_single.contains("should_not_appear_in_outline"));

    let dir_outline = engine.outline(Some("src/orders"), 4000);
    assert_eq!(dir_outline.files_total, 2);
    assert_eq!(dir_outline.files.len(), 2);
    assert!(dir_outline.files.iter().all(|f| f.file.starts_with("src/orders/")));
    let internal = dir_outline
        .files
        .iter()
        .find(|f| f.file == "src/orders/orders.internal.ts")
        .expect("internal file");
    assert_eq!(internal.symbols.len(), 2);

    let budgeted = engine.outline(None, 55);
    assert_eq!(budgeted.files_total, 3);
    assert!(budgeted.truncated);
    assert!(budgeted.files.len() < budgeted.files_total);
}

fn setup_cross_repo_route_fixtures(back_root: &Path, front_root: &Path) {
    write_repo_file(
        back_root,
        "src/orders/orders.controller.ts",
        "import { Controller, Get } from '@nestjs/common';\n\
         @Controller('orders')\n\
         export class OrdersController {\n\
           @Get(':id')\n\
           findOne(id: string) { return { id }; }\n\
         }\n",
    );
    write_repo_file(
        front_root,
        "src/app/orders.service.ts",
        "export class OrdersService {\n\
           private api = 'https://api.example.com';\n\
           constructor(private http: any) {}\n\
           getOrder(id: string) {\n\
             return this.http.get(`${this.api}/orders/${id}`);\n\
           }\n\
         }\n",
    );
    write_repo_file(
        front_root,
        "src/app/orders.fetch.ts",
        "const API = 'https://api.example.com';\n\
         export function fetchOrder(id: string) {\n\
           return fetch(`${API}/orders/${id}`);\n\
         }\n",
    );
    write_repo_file(
        front_root,
        "src/app/unrelated.service.ts",
        "export class UnrelatedService { ping() { return 'pong'; } }\n",
    );
}

#[test]
fn cross_repo_http_route_blast_radius_connects_nestjs_and_frontend() {
    let back_dir = TempDir::new().expect("back");
    let front_dir = TempDir::new().expect("front");
    setup_cross_repo_route_fixtures(back_dir.path(), front_dir.path());

    let back = CodeGraphEngine::build(back_dir.path(), AnvilConfig::default()).expect("back");
    let front = CodeGraphEngine::build(front_dir.path(), AnvilConfig::default()).expect("front");

    let merged = merge_graphs(&[
        ("fliiper-backend", &back.graph),
        ("fliiper-front", &front.graph),
    ]);

    assert!(
        merged.node_by_id("route:GET /orders/:param").is_some_and(|(_, n)| n.kind == NodeKind::Semantic),
        "expected unprefixed route:GET /orders/:param semantic node in merged graph"
    );

    let report = compute_blast_radius(
        &merged,
        &["fliiper-backend/src/orders/orders.controller.ts".to_owned()],
        2,
    );
    let impacted: Vec<&str> = report.impacted_files.iter().map(|i| i.file.as_str()).collect();
    assert!(impacted.contains(&"fliiper-front/src/app/orders.service.ts"), "got: {impacted:?}");
    assert!(impacted.contains(&"fliiper-front/src/app/orders.fetch.ts"), "got: {impacted:?}");
    assert!(!impacted.contains(&"fliiper-front/src/app/unrelated.service.ts"), "got: {impacted:?}");
}

#[test]
fn build_ts_typecheck_command_prefers_tsgo_and_puts_tsbuildinfo_off_repo() {
    let log_dir = TempDir::new().expect("log_dir");
    let work_tsgo = TempDir::new().expect("work_tsgo");
    let work_tsc = TempDir::new().expect("work_tsc");

    create_mock_bin(work_tsgo.path(), "tsgo");
    create_mock_bin(work_tsgo.path(), "tsc");
    create_mock_bin(work_tsc.path(), "tsc");

    let (bin1, args1, tool1) =
        build_ts_typecheck_command(log_dir.path(), work_tsgo.path()).expect("tsgo command");
    assert_eq!(tool1, "tsgo");
    assert!(bin1.to_string_lossy().to_ascii_lowercase().contains("tsgo"));
    assert_eq!(args1[0], "--noEmit");
    assert_eq!(args1[1], "--incremental");
    assert_eq!(args1[2], "--tsBuildInfoFile");

    let info_arg = Path::new(&args1[3]);
    let expected_state_dir = log_dir.path().join(".harness").join(".state");
    assert!(expected_state_dir.is_dir());
    assert!(info_arg.starts_with(&expected_state_dir));
    assert!(
        info_arg
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("tsbuildinfo-") && n.ends_with(".tsbuildinfo"))
    );
    assert!(!info_arg.starts_with(work_tsgo.path()));

    let (bin2, args2, tool2) =
        build_ts_typecheck_command(log_dir.path(), work_tsc.path()).expect("tsc fallback");
    assert_eq!(tool2, "tsc");
    assert!(bin2.to_string_lossy().to_ascii_lowercase().contains("tsc"));
    assert_ne!(args1[3], args2[3], "distinct work_dir hashes must not collide");
}

#[test]
fn run_impacted_typecheck_retries_plain_no_emit_when_incremental_rejected() {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();

    write_repo_file(root, "src/app.ts", "export const x = 1;\n");
    let bin_dir = root.join("node_modules").join(".bin");
    fs::create_dir_all(&bin_dir).expect("create .bin");
    if cfg!(windows) {
        let p = bin_dir.join("tsc.cmd");
        fs::write(
            &p,
            "@echo off\r\n\
             echo %* | findstr /C:\"incremental\" >nul\r\n\
             if not errorlevel 1 (\r\n\
               echo error TS5069: incremental rejected\r\n\
               exit /b 1\r\n\
             )\r\n\
             exit /b 0\r\n",
        )
        .expect("write tsc.cmd");
    } else {
        let p = bin_dir.join("tsc");
        fs::write(
            &p,
            "#!/bin/sh\n\
             case \"$*\" in\n\
               *incremental*) echo \"error TS5069: incremental rejected\"; exit 1 ;;\n\
               *) exit 0 ;;\n\
             esac\n",
        )
        .expect("write tsc");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&p).expect("meta").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&p, perms).expect("chmod");
        }
    }

    let cfg = AnvilConfig {
        stacks: vec![StackKind::TypeScript],
        ..Default::default()
    };
    let engine = CodeGraphEngine::build(root, cfg).expect("build");
    let outputs = engine
        .run_impacted_typecheck(&["src/app.ts".to_owned()])
        .expect("run_impacted_typecheck");
    assert_eq!(outputs.len(), 1);
    assert!(outputs[0].passed, "expected fallback --noEmit to pass: {:?}", outputs[0]);
    assert_eq!(outputs[0].exit_code, 0);
}
