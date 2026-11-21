//! Regression tests for Rust AST extraction/rules, `anvil-allow`, and TS hardening.

use std::path::Path;

use anvil_ast::analyze_file;
use anvil_core::config::AnvilConfig;
use anvil_core::traits::{EdgeKind, Language, SymbolKind};

fn cfg() -> AnvilConfig {
    AnvilConfig::default()
}

const RUST_SAMPLE: &str = r#"
use std::collections::HashMap;
use crate::core::{Config, Runner as AppRunner};

mod helpers;

pub const MAX_ITEMS: usize = 64;
pub type ItemId = u64;

pub enum Status { Active, Inactive }
pub trait Greeter { fn greet(&self); }
pub struct User { pub id: ItemId }

impl Greeter for User {
    fn greet(&self) {
        let _m = HashMap::<String, String>::new();
        helper_fn();
        self.inner_call();
        make_chain()
            .step_one()
            .step_two();
    }
}

pub fn helper_fn() {}
"#;

#[test]
fn extracts_rust_symbols() {
    let facts = analyze_file(Path::new("src/user.rs"), RUST_SAMPLE, &cfg()).expect("rust");
    assert_eq!(facts.language, Language::Rust);
    assert!(facts.symbols.iter().any(|s| s.name == "helpers" && s.kind == SymbolKind::Module));
    assert!(facts.symbols.iter().any(|s| s.name == "MAX_ITEMS" && s.kind == SymbolKind::Constant));
    assert!(facts.symbols.iter().any(|s| s.name == "ItemId" && s.kind == SymbolKind::TypeAlias));
    assert!(facts.symbols.iter().any(|s| s.name == "Status" && s.kind == SymbolKind::Class));
    assert!(facts.symbols.iter().any(|s| s.name == "Greeter" && s.kind == SymbolKind::Interface));
    assert!(facts.symbols.iter().any(|s| s.name == "User" && s.kind == SymbolKind::Class));
    assert!(facts.symbols.iter().any(|s| s.name == "helper_fn" && s.kind == SymbolKind::Function));
    assert!(facts.symbols.iter().any(|s| s.qualified_name == "User::greet" && s.kind == SymbolKind::Method));
    assert!(facts.symbols.iter().any(|s| s.name == "impl Greeter for User"));
}

#[test]
fn extracts_rust_edges_and_calls() {
    let facts = analyze_file(Path::new("src/user.rs"), RUST_SAMPLE, &cfg()).expect("rust");
    assert!(facts.edges.iter().any(|e| e.kind == EdgeKind::Imports && e.target_path_or_symbol == "std::collections::HashMap"));
    assert!(facts.edges.iter().any(|e| e.kind == EdgeKind::Imports && e.target_path_or_symbol == "crate::core::Config"));
    assert!(facts.edges.iter().any(|e| e.kind == EdgeKind::Imports && e.target_path_or_symbol == "crate::core::Runner"));
    assert!(facts.edges.iter().any(|e| e.kind == EdgeKind::Imports && e.target_path_or_symbol == "helpers"));
    assert!(facts.edges.iter().any(|e| e.kind == EdgeKind::Implements && e.source_symbol.as_deref() == Some("User") && e.target_path_or_symbol == "Greeter"));
    assert!(facts.edges.iter().any(|e| e.kind == EdgeKind::Calls && e.target_path_or_symbol == "helper_fn"));
    assert!(facts.edges.iter().any(|e| e.kind == EdgeKind::Calls && e.target_path_or_symbol == "self.inner_call"));
    assert!(facts.edges.iter().any(|e| e.kind == EdgeKind::Calls && e.target_path_or_symbol == "step_two"));
    assert!(facts.edges.iter().filter(|e| e.kind == EdgeKind::Calls).all(|e| !e.target_path_or_symbol.contains('\n') && e.target_path_or_symbol.len() <= 80));
}

#[test]
fn rust_tier0_blocks_todo_allow_and_unsafe_unless_anvil_allowed() {
    let bad = r#"
// TODO: remove workaround
#[allow(dead_code)]
pub fn legacy() {
    unsafe { core::hint::unreachable_unchecked(); }
}
"#;
    let res = analyze_file(Path::new("src/legacy.rs"), bad, &cfg()).expect("rust bad");
    assert_eq!(res.diagnostics.len(), 3, "diags: {:?}", res.diagnostics);
    assert!(res.diagnostics.iter().any(|d| d.rule == "no-todo-comments"));
    assert_eq!(res.diagnostics.iter().filter(|d| d.rule == "no-escape-hatches").count(), 2);

    let allowed = r#"
// anvil-allow(no-todo-comments): tracked in sprint [ANV-10]
// TODO: remove workaround
// anvil-allow(no-rust-allow): FFI ABI shim [ANV-11]
#[allow(dead_code)]
pub fn legacy() {
    // anvil-allow(no-unsafe): pointer guaranteed aligned [ANV-12]
    unsafe { core::hint::unreachable_unchecked(); }
}
"#;
    let ok = analyze_file(Path::new("src/legacy.rs"), allowed, &cfg()).expect("rust ok");
    assert!(ok.diagnostics.is_empty(), "unexpected: {:?}", ok.diagnostics);
}

#[test]
fn anvil_allow_requires_comment_node_and_matching_rule_id() {
    let string_spoof = r#"
export const fake = "anvil-allow(all): spoofed in string [SEC-1]";
// TODO: real todo must still fail
// @ts-ignore
export const x = 1;
"#;
    let spoof = analyze_file(Path::new("src/spoof.ts"), string_spoof, &cfg()).expect("spoof");
    assert_eq!(spoof.diagnostics.len(), 2, "diags: {:?}", spoof.diagnostics);

    let wrong_rule = r#"
// anvil-allow(other-rule): mismatched rule [SEC-2]
// TODO: still fails
// anvil-allow(no-important): wrong hatch [SEC-3]
// @ts-ignore
export const y = 2;
"#;
    let wrong = analyze_file(Path::new("src/wrong.ts"), wrong_rule, &cfg()).expect("wrong");
    assert_eq!(wrong.diagnostics.len(), 2, "diags: {:?}", wrong.diagnostics);

    let valid_todo = r#"
// anvil-allow(no-todo-comments): follow-up ticket [SEC-4]
// TODO: refactor later
// FIXME: inline allow (anvil-allow(comments): same line [SEC-5])
// anvil-allow(all): emergency hotfix [SEC-6]
// @ts-ignore
export const z = 3;
"#;
    let valid = analyze_file(Path::new("src/valid.ts"), valid_todo, &cfg()).expect("valid");
    assert!(valid.diagnostics.is_empty(), "unexpected: {:?}", valid.diagnostics);
}

#[test]
fn biome_ignore_and_python_type_ignore_blocked_unless_allowed() {
    let ts_biome = "// biome-ignore lint/suspicious/noExplicitAny: skip\nexport const a = 1;\n";
    let r1 = analyze_file(Path::new("src/a.ts"), ts_biome, &cfg()).expect("biome ts");
    assert_eq!(r1.diagnostics.len(), 1);
    assert_eq!(r1.diagnostics[0].rule, "no-escape-hatches");

    let ts_biome_ok = "// anvil-allow(no-biome-ignore): vendor generated [B-1]\n// biome-ignore lint/suspicious/noExplicitAny: skip\nexport const a = 1;\n";
    let r2 = analyze_file(Path::new("src/a.ts"), ts_biome_ok, &cfg()).expect("biome ok");
    assert!(r2.diagnostics.is_empty());

    let py_type = "x: int = 'oops'  # type: ignore[assignment]\n";
    let r3 = analyze_file(Path::new("app/t.py"), py_type, &cfg()).expect("py type");
    assert_eq!(r3.diagnostics.len(), 1);
    assert_eq!(r3.diagnostics[0].rule, "no-escape-hatches");

    let py_type_ok = "# anvil-allow(no-type-ignore): third-party stub [PY-9]\nx: int = 'oops'  # type: ignore\n";
    let r4 = analyze_file(Path::new("app/t.py"), py_type_ok, &cfg()).expect("py ok");
    assert!(r4.diagnostics.is_empty());
}

#[test]
fn tsx_skips_dom_props_sanitizes_multiline_calls_and_parses_decorators_via_ast() {
    let tsx = r#"
export function View() {
  const data = fetch('/api/orders')
    .then((res) => res.json())
    .catch((err) => {
      console.error(err);
      return null;
    });
  return (
    <div className="x">
      <span id="s" />
      <CheckoutButton variant="primary" />
      <Form.Input label="Email" />
    </div>
  );
}
"#;
    let facts = analyze_file(Path::new("src/View.tsx"), tsx, &cfg()).expect("tsx");
    assert!(!facts.edges.iter().any(|e| e.kind == EdgeKind::UsesProp && e.target_path_or_symbol.starts_with("div.")));
    assert!(!facts.edges.iter().any(|e| e.kind == EdgeKind::UsesProp && e.target_path_or_symbol.starts_with("span.")));
    assert!(facts.edges.iter().any(|e| e.kind == EdgeKind::UsesProp && e.target_path_or_symbol == "CheckoutButton.variant"));
    assert!(facts.edges.iter().any(|e| e.kind == EdgeKind::UsesProp && e.target_path_or_symbol == "Form.Input.label"));

    let calls: Vec<&str> = facts
        .edges
        .iter()
        .filter(|e| e.kind == EdgeKind::Calls)
        .map(|e| e.target_path_or_symbol.as_str())
        .collect();
    assert!(calls.iter().all(|c| !c.contains('\n') && c.len() <= 80), "bad calls: {calls:?}");
    assert!(calls.contains(&"fetch"));
    assert!(calls.contains(&"then"));
    assert!(calls.contains(&"res.json"));
    assert!(calls.contains(&"catch"));
    assert!(calls.contains(&"console.error"));

    let ng = r#"
@Component({
  // selector: 'comment-fake',
  selector: 'real-card',
  templateUrl: './real-card.html'
})
export class RealCardComponent {}
"#;
    let ng_facts = analyze_file(Path::new("src/real.ts"), ng, &cfg()).expect("ng ast");
    assert!(ng_facts.symbols.iter().any(|s| s.name == "real-card" && s.kind == SymbolKind::Component));
    assert!(!ng_facts.symbols.iter().any(|s| s.name == "comment-fake"));
    assert!(ng_facts.edges.iter().any(|e| e.kind == EdgeKind::RendersTemplate && e.target_path_or_symbol == "./real-card.html"));
}
