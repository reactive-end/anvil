//! Integration tests for `.vue`, `.svelte`, and `.astro` Single-File Component analysis.

use std::path::Path;

use anvil_ast::sfc::is_sfc_path;
use anvil_ast::{analyze_file, detect_language};
use anvil_core::config::AnvilConfig;
use anvil_core::traits::{EdgeKind, FileAstFacts, Language};

fn cfg() -> AnvilConfig {
    AnvilConfig::default()
}

fn has_diag(facts: &FileAstFacts, rule: &str, line: u32, needle: &str) -> bool {
    facts
        .diagnostics
        .iter()
        .any(|d| d.rule == rule && d.line == line && d.message.contains(needle))
}

fn has_edge(facts: &FileAstFacts, kind: EdgeKind, target: &str, line: u32) -> bool {
    facts
        .edges
        .iter()
        .any(|e| e.kind == kind && e.target_path_or_symbol == target && e.line == line)
}

#[test]
fn is_sfc_path_recognizes_vue_svelte_astro_and_preserves_detect_language() {
    assert!(is_sfc_path(Path::new("src/App.vue")));
    assert!(is_sfc_path(Path::new("src/Counter.SVELTE")));
    assert!(is_sfc_path(Path::new("src/pages/index.Astro")));
    assert!(!is_sfc_path(Path::new("src/app.ts")));
    assert!(!is_sfc_path(Path::new("src/index.html")));

    assert_eq!(detect_language(Path::new("src/App.vue")), Language::Other);
    assert_eq!(detect_language(Path::new("src/Counter.svelte")), Language::Other);
    assert_eq!(detect_language(Path::new("src/pages/index.astro")), Language::Other);
}

#[test]
fn vue_sfc_extracts_edges_and_diagnostics_with_exact_1based_lines() {
    let src = "\
<template>
  <!-- TODO: move header -->
  <div class=\"orders\">Hello</div>
</template>
<script setup lang=\"ts\">
import { api } from '@/api/client'
// @ts-ignore
const bad: any = 1
await fetch('/api/v1/orders', { method: 'GET' })
</script>
<style scoped>
.orders {
  color: red !important;
}
</style>
";
    let facts = analyze_file(Path::new("src/OrdersView.vue"), src, &cfg()).expect("analyze vue");
    assert_eq!(facts.language, Language::TypeScript);
    assert!(has_edge(&facts, EdgeKind::Imports, "@/api/client", 6));
    assert!(has_edge(
        &facts,
        EdgeKind::CallsRoute,
        "route:GET /api/v1/orders",
        9
    ));
    assert!(has_diag(&facts, "no-todo-comments", 2, "TODO"));
    assert!(has_diag(&facts, "no-escape-hatches", 7, "@ts-ignore"));
    assert!(has_diag(&facts, "no-explicit-any", 8, "any"));
    assert!(has_diag(&facts, "no-escape-hatches", 13, "!important"));
}

#[test]
fn vue_sfc_supports_anvil_allow_on_preceding_line_and_multiple_script_blocks() {
    let src = "\
<template><div /></template>
<script lang=\"ts\">
export interface OrderDto { id: string }
</script>
<script setup lang=\"ts\">
// anvil-allow(no-explicit-any): legacy DTO [T-1]
const legacy: any = {}
</script>
";
    let facts = analyze_file(Path::new("src/Legacy.vue"), src, &cfg()).expect("analyze vue allow");
    assert_eq!(facts.language, Language::TypeScript);
    assert!(
        facts.diagnostics.is_empty(),
        "expected no diagnostics when anvil-allow is present: {:?}",
        facts.diagnostics
    );
    assert!(
        facts.symbols.iter().any(|s| s.name == "OrderDto" && s.line == 3),
        "expected OrderDto symbol from first script block at line 3: {:?}",
        facts.symbols
    );
}

#[test]
fn vue_js_and_midline_script_preserve_language_and_column() {
    let src = "<template><p>Hi</p></template>\n<script>const x = 1; // @ts-ignore</script>\n";
    let facts = analyze_file(Path::new("src/Plain.vue"), src, &cfg()).expect("analyze plain vue");
    assert_eq!(facts.language, Language::JavaScript);
    let diag = facts
        .diagnostics
        .iter()
        .find(|d| d.rule == "no-escape-hatches")
        .expect("expected @ts-ignore diagnostic");
    assert_eq!(diag.line, 2);
    assert_eq!(diag.column, 25);
}

#[test]
fn svelte_and_astro_frontmatter_preserve_exact_lines_and_edges() {
    let svelte_src = "\
<script lang=\"ts\">
  import { store } from './store'
  const count: any = 0
</script>
<main>{count}</main>
<style lang=\"scss\">
  .btn { color: blue !important; }
</style>
";
    let svelte =
        analyze_file(Path::new("src/Counter.svelte"), svelte_src, &cfg()).expect("analyze svelte");
    assert_eq!(svelte.language, Language::TypeScript);
    assert!(has_edge(&svelte, EdgeKind::Imports, "./store", 2));
    assert!(has_diag(&svelte, "no-explicit-any", 3, "any"));
    assert!(has_diag(&svelte, "no-escape-hatches", 7, "!important"));

    let astro_src = "\
---
import Layout from '@/layouts/Base.astro'
const raw: any = await fetch('/api/v1/health', { method: 'GET' })
---
<Layout>
  <!-- TODO: localize title -->
  <h1>Health</h1>
</Layout>
";
    let astro =
        analyze_file(Path::new("src/pages/index.astro"), astro_src, &cfg()).expect("analyze astro");
    assert_eq!(astro.language, Language::TypeScript);
    assert!(has_edge(
        &astro,
        EdgeKind::Imports,
        "@/layouts/Base.astro",
        2
    ));
    assert!(has_edge(
        &astro,
        EdgeKind::CallsRoute,
        "route:GET /api/v1/health",
        3
    ));
    assert!(has_diag(&astro, "no-explicit-any", 3, "any"));
    assert!(has_diag(&astro, "no-todo-comments", 6, "TODO"));
}

#[test]
fn sfc_handles_multibyte_utf8_characters_without_panicking() {
    let vue_src = "\
<template>
  <!-- © Sección española: ñ á 🚀 -->
  <p>Diseño mañana 🚀 — cálculo</p>
</template>
<script setup lang=\"ts\">
import { api } from '@/api/diseño'
export const título = 'Año © 🚀';
</script>
";
    let vue = analyze_file(Path::new("src/Espanol.vue"), vue_src, &cfg()).expect("vue utf8");
    assert_eq!(vue.language, Language::TypeScript);
    assert!(has_edge(&vue, EdgeKind::Imports, "@/api/diseño", 6));

    let svelte_src = "<p>Ñandú 🚀</p>\n<script lang=\"ts\">export const ok = 'sí';</script>\n";
    let svelte =
        analyze_file(Path::new("src/Utf8.svelte"), svelte_src, &cfg()).expect("svelte utf8");
    assert_eq!(svelte.language, Language::TypeScript);

    let astro_src = "---\nexport const msg = 'hola ñ 🚀';\n---\n<h1>Diseño ©</h1>\n";
    let astro =
        analyze_file(Path::new("src/Utf8.astro"), astro_src, &cfg()).expect("astro utf8");
    assert_eq!(astro.language, Language::TypeScript);
}
