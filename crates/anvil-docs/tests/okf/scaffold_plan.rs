use anvil_docs::bundle::Bundle;
use anvil_docs::scaffold::{ModulePlan, plan};
use anvil_graph::model::CodeGraph;

fn graph(files: &[&str]) -> CodeGraph {
    let mut graph = CodeGraph::new("repo");
    for file in files {
        graph.file_hashes.insert((*file).to_owned(), "h".to_owned());
    }
    graph
}

fn project(code_root: &str) -> Bundle {
    Bundle {
        project: "p".to_owned(),
        dir: "/tmp/none".into(),
        code_root: code_root.to_owned(),
        nested: Vec::new(),
    }
}

fn modules(plans: &[ModulePlan]) -> Vec<&str> {
    plans.iter().map(|p| p.module.as_str()).collect()
}

#[test]
fn nest_layout_gives_one_module_per_src_folder_with_its_layers() {
    let g = graph(&[
        "src/main.ts",
        "src/orders/domain/order.ts",
        "src/orders/application/create.ts",
        "src/orders/controller/orders.controller.ts",
        "src/billing/billing.service.ts",
        "test/orders.e2e.ts",
    ]);
    let plans = plan(&g, &project(""), None);
    assert_eq!(modules(&plans), ["billing", "orders"]);
    let orders = &plans[1];
    assert_eq!(orders.glob, "src/orders/**");
    assert_eq!(orders.files, 3);
    let layers: Vec<&str> = orders.layers.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(layers, ["application", "controller", "domain"]);
    assert!(plans[0].layers.is_empty());
}

#[test]
fn rust_workspace_defaults_to_folders_under_each_crate_src() {
    let g = graph(&[
        "crates/core/src/lib.rs",
        "crates/core/src/config/mod.rs",
        "crates/core/src/git/diff.rs",
        "crates/core/tests/it.rs",
        "crates/cli/src/main.rs",
    ]);
    let plans = plan(&g, &project(""), None);
    assert_eq!(modules(&plans), ["cli", "core/config", "core/git"]);
    assert_eq!(plans[1].glob, "crates/core/src/config/**");
    assert_eq!(plans[0].glob, "crates/cli/**");
}

#[test]
fn depth_one_makes_crates_the_modules() {
    let g = graph(&[
        "crates/core/src/lib.rs",
        "crates/core/src/config/mod.rs",
        "crates/cli/src/main.rs",
    ]);
    let plans = plan(&g, &project(""), Some(1));
    assert_eq!(modules(&plans), ["cli", "core"]);
    assert_eq!(plans[1].glob, "crates/core/**");
    assert_eq!(plans[1].files, 2);
}

#[test]
fn subrepo_bundles_only_see_their_own_files_with_repo_relative_globs() {
    let g = graph(&["apps/api/src/orders/a.ts", "apps/web/src/home/b.ts"]);
    let plans = plan(&g, &project("apps/api"), None);
    assert_eq!(modules(&plans), ["orders"]);
    assert_eq!(plans[0].glob, "apps/api/src/orders/**");
}

#[test]
fn a_root_bundle_leaves_nested_subrepo_files_to_their_own_bundles() {
    let g = graph(&["tools/a.ts", "apps/api/src/orders/a.ts"]);
    let mut root = project("");
    root.nested = vec!["apps/api".to_owned()];
    let plans = plan(&g, &root, Some(1));
    assert_eq!(modules(&plans), ["tools"]);
}
