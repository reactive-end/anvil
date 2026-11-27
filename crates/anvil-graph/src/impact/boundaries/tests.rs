use anvil_core::config::AnvilConfig;
use anvil_core::config::architecture::ArchitectureStyle;
use anvil_core::config::architecture::rules::violation_target;
use anvil_core::traits::Diagnostic;

use super::check_boundaries;
use crate::model::{CodeGraph, CompactEdge, GraphEdgeKind, GraphNode, NodeId, NodeKind};

/// A hand-built import graph: files plus `import` edges to files or external specifiers.
struct Fixture {
    graph: CodeGraph,
}

impl Fixture {
    fn new() -> Self {
        Self { graph: CodeGraph::new("repo") }
    }

    fn file(&mut self, path: &str) -> NodeId {
        if let Some((nid, _)) = self.graph.node_by_id(&CodeGraph::file_node_id(path)) {
            return nid;
        }
        let node = GraphNode {
            id: CodeGraph::file_node_id(path),
            kind: NodeKind::File,
            label: path.to_owned(),
            file: path.to_owned(),
            line: 1,
            end_line: 1,
            language: None,
            symbol_kind: None,
            signature: None,
            exported: false,
            community: None,
        };
        let nid = self.graph.insert_node_with_slot(node, None);
        self.graph.file_nodes.entry(path.to_owned()).or_default().push(nid);
        nid
    }

    fn import(&mut self, from: &str, to: &str, line: u32) -> &mut Self {
        let (source, target) = (self.file(from), self.file(to));
        let edge = CompactEdge { source, target, kind: GraphEdgeKind::Imports, line };
        self.graph.push_edge(edge, true);
        self
    }

    fn import_package(&mut self, from: &str, specifier: &str, line: u32) -> &mut Self {
        let source = self.file(from);
        let target = self.graph.intern_external(specifier);
        let edge = CompactEdge { source, target, kind: GraphEdgeKind::Imports, line };
        self.graph.push_edge(edge, true);
        self
    }

    fn check(&self, style: ArchitectureStyle) -> Vec<Diagnostic> {
        self.check_scoped(style, &[])
    }

    fn check_scoped(&self, style: ArchitectureStyle, scope: &[String]) -> Vec<Diagnostic> {
        let mut config = AnvilConfig::default();
        config.set_architecture(style, None, None).expect("set style");
        check_boundaries(&self.graph, &config, scope)
    }
}

fn summary(diags: &[Diagnostic]) -> Vec<(String, u32, String, String)> {
    let mut rows: Vec<_> = diags
        .iter()
        .map(|d| {
            let target = violation_target(&d.message).expect("target in message").to_owned();
            (d.file.clone(), d.line, target, d.rule.clone())
        })
        .collect();
    rows.sort();
    rows
}

#[test]
fn domain_importing_nestjs_common_is_flagged_but_application_may_use_it() {
    let mut fx = Fixture::new();
    fx.import_package("src/orders/domain/order.ts", "@nestjs/common", 3).import_package(
        "src/orders/application/create-order.use-case.ts",
        "@nestjs/common",
        1,
    );

    let rows = summary(&fx.check(ArchitectureStyle::Hexagonal));

    assert_eq!(
        rows,
        [(
            "src/orders/domain/order.ts".to_owned(),
            3,
            "@nestjs/common".to_owned(),
            "boundary:arch:hexagonal:domain-no-framework".to_owned()
        )]
    );
}

#[test]
fn package_subpaths_match_and_lookalike_packages_do_not() {
    let mut fx = Fixture::new();
    fx.import_package("src/orders/domain/a.ts", "typeorm/repository", 1)
        .import_package("src/orders/domain/b.ts", "@nestjs/common/decorators", 2)
        .import_package("src/orders/domain/c.ts", "pg-promise", 3)
        .import_package("src/orders/domain/d.ts", "@nestjsx/crud", 4)
        .import_package("src/orders/domain/e.ts", "./local-helper", 5)
        .import_package("src/orders/application/f.ts", "typeorm/repository", 6)
        .import_package("src/orders/application/g.ts", "ioredis", 7);

    let rows = summary(&fx.check(ArchitectureStyle::Hexagonal));

    let flagged: Vec<(&str, &str)> =
        rows.iter().map(|(file, _, target, _)| (file.as_str(), target.as_str())).collect();
    assert_eq!(
        flagged,
        [
            ("src/orders/application/f.ts", "typeorm/repository"),
            ("src/orders/domain/a.ts", "typeorm/repository"),
            ("src/orders/domain/b.ts", "@nestjs/common/decorators"),
        ]
    );
}

#[test]
fn layer_rules_flag_domain_to_infrastructure_with_the_import_line() {
    let mut fx = Fixture::new();
    fx.import("src/orders/domain/order.ts", "src/orders/infrastructure/pg-order.repository.ts", 7)
        .import("src/orders/domain/order.ts", "src/orders/domain/order-status.ts", 2)
        .import("src/orders/application/create.ts", "src/orders/domain/order.ts", 1)
        .import(
            "src/orders/infrastructure/pg-order.repository.ts",
            "src/orders/domain/order.ts",
            1,
        );

    let diags = fx.check(ArchitectureStyle::Hexagonal);

    let rows = summary(&diags);
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].1, 7);
    assert_eq!(rows[0].3, "boundary:arch:hexagonal:domain-no-infrastructure");
    let suggestion = diags[0].suggestion.as_deref().expect("reason");
    assert!(suggestion.contains("port"), "reason must teach the fix: {suggestion}");
}

#[test]
fn module_isolation_allows_public_paths_and_flags_internals() {
    let mut fx = Fixture::new();
    fx.import("src/orders/application/create.ts", "src/customers/domain/customer.ts", 1)
        .import("src/orders/application/create.ts", "src/customers/application/get.ts", 2)
        .import("src/orders/orders.module.ts", "src/customers/customers.module.ts", 3)
        .import("src/orders/application/create.ts", "src/customers/infrastructure/orm.ts", 4)
        .import("src/orders/application/create.ts", "src/customers/controller/ctrl.ts", 5)
        .import("src/orders/application/create.ts", "src/shared/infrastructure/db.ts", 6)
        .import("src/app.module.ts", "src/customers/infrastructure/orm.ts", 7)
        .import("src/orders/application/create.ts", "src/orders/infrastructure/own.ts", 8)
        .import("src/shared/database/db.config.ts", "src/customers/infrastructure/orm.ts", 9);

    let rows = summary(&fx.check(ArchitectureStyle::Hexagonal));

    let modules: Vec<(u32, &str)> = rows
        .iter()
        .filter(|r| r.3.ends_with("module-boundaries"))
        .map(|r| (r.1, r.2.as_str()))
        .collect();
    assert_eq!(
        modules,
        [(4, "src/customers/infrastructure/orm.ts"), (5, "src/customers/controller/ctrl.ts")]
    );
}

#[test]
fn modular_monolith_only_allows_the_public_api() {
    let mut fx = Fixture::new();
    fx.import("src/billing/service.ts", "src/users/index.ts", 1)
        .import("src/billing/service.ts", "src/users/users.module.ts", 2)
        .import("src/billing/service.ts", "src/users/internal/repo.ts", 3);

    let rows = summary(&fx.check(ArchitectureStyle::ModularMonolith));

    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!(rows[0].2, "src/users/internal/repo.ts");
}

#[test]
fn feature_sliced_blocks_upward_imports_and_cross_slice_imports() {
    let mut fx = Fixture::new();
    fx.import("src/features/login/ui.ts", "src/pages/home/page.ts", 1)
        .import("src/features/login/ui.ts", "src/features/signup/model.ts", 2)
        .import("src/features/login/ui.ts", "src/entities/user/model.ts", 3)
        .import("src/pages/home/page.ts", "src/features/login/ui.ts", 4);

    let rows = summary(&fx.check(ArchitectureStyle::FeatureSliced));

    let rules: Vec<(&str, u32)> = rows.iter().map(|r| (r.3.as_str(), r.1)).collect();
    assert_eq!(
        rules,
        [
            ("boundary:arch:feature-sliced:features-imports-downward", 1),
            ("boundary:arch:feature-sliced:features-slices-isolated", 2),
        ]
    );
}

#[test]
fn event_driven_isolates_handlers_of_other_contexts_but_allows_their_events() {
    let mut fx = Fixture::new();
    fx.import("src/billing/handlers/on-order.ts", "src/orders/domain/order.ts", 1)
        .import("src/billing/handlers/on-order.ts", "src/orders/events/order-placed.event.ts", 2)
        .import("src/billing/service.ts", "src/orders/domain/order.ts", 3);

    let rows = summary(&fx.check(ArchitectureStyle::EventDriven));

    assert_eq!(rows.len(), 1, "{rows:?}");
    assert_eq!((rows[0].0.as_str(), rows[0].1), ("src/billing/handlers/on-order.ts", 1));
}

#[test]
fn tests_and_scope_filters_apply_and_user_boundaries_still_work() {
    let mut fx = Fixture::new();
    fx.import_package("src/orders/domain/order.ts", "typeorm", 1)
        .import_package("src/orders/domain/order.spec.ts", "typeorm", 1)
        .import_package("src/customers/domain/customer.ts", "typeorm", 1);

    let all = fx.check(ArchitectureStyle::Hexagonal);
    assert_eq!(all.len(), 2, "spec files are not held to the rules: {all:?}");

    let scoped = fx.check_scoped(
        ArchitectureStyle::Hexagonal,
        &["./src/customers/domain/customer.ts".to_owned()],
    );
    assert_eq!(scoped.len(), 1);
    assert_eq!(scoped[0].file, "src/customers/domain/customer.ts");

    let mut config = AnvilConfig::default();
    config.boundaries.push(anvil_core::config::BoundaryRule {
        name: "mine".to_owned(),
        from_glob: "src/orders/domain/**".to_owned(),
        forbid_glob: "src/customers/**".to_owned(),
        reason: None,
    });
    let mut fx2 = Fixture::new();
    fx2.import("src/orders/domain/order.ts", "src/customers/domain/customer.ts", 1);
    let diags = check_boundaries(&fx2.graph, &config, &[]);
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].rule, "boundary:mine");
}

#[test]
fn no_style_and_no_rules_means_no_work() {
    let mut fx = Fixture::new();
    fx.import_package("src/orders/domain/order.ts", "typeorm", 1);
    assert!(fx.check(ArchitectureStyle::None).is_empty());
}
