use std::fs;

use anvil_adapters::claude_code;
use anvil_adapters::contract::{HookEvent, PermissionAction};
use anvil_graph::CodeGraphEngine;
use serde_json::{Value, json};
use tempfile::TempDir;

use super::super::roles_shell::{why_not_executor, why_not_read_only};
use super::*;

const FORMATTERS: &str = "export function formatDate(d: Date): string {\n  return d.toISOString();\n}\n\nexport function formatMoney(n: number): string {\n  return n.toFixed(2);\n}\n";
const CONCEPT: &str = "---\ntype: Code Module\ntitle: Invoicing\ndescription: How invoices are totalled\n---\n# Invoicing\n\nInvoices are totalled by the ledger service.\n";
const CONCEPT_PATH: &str = ".harness/docs/shop/invoicing/overview.md";
const HINT: &str =
    "Graph and docs were checked first; repeat the search if you still need the code.";

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

/// A repository with `.harness/`, a cached graph, and one documented concept.
fn repo() -> TempDir {
    let dir = TempDir::new().unwrap();
    write(dir.path(), "src/format.ts", FORMATTERS);
    write(dir.path(), CONCEPT_PATH, CONCEPT);
    CodeGraphEngine::build(dir.path(), AnvilConfig::default()).unwrap().save().unwrap();
    dir
}

fn call(agent: Option<&str>, tool: &str, tool_input: Value) -> NormalizedHookInput {
    let mut payload = json!({ "session_id": "s1", "tool_name": tool, "tool_input": tool_input });
    if let Some(id) = agent {
        payload["agent_id"] = json!(id);
    }
    claude_code::parse_input(&payload.to_string(), Some(HookEvent::PreToolUse)).unwrap()
}

fn shell(cmd: &str) -> NormalizedHookInput {
    call(None, "Bash", json!({ "command": cmd }))
}

fn grep(pattern: &str) -> NormalizedHookInput {
    call(None, "Grep", json!({ "pattern": pattern }))
}

fn guard(dir: &TempDir, input: &NormalizedHookInput) -> Option<HookOutcome> {
    check_search_guard(dir.path(), &AnvilConfig::default(), input)
}

fn guard_in(
    dir: &TempDir,
    mode: SearchCascadeMode,
    input: &NormalizedHookInput,
) -> Option<HookOutcome> {
    let mut config = AnvilConfig::default();
    config.search.cascade = mode;
    check_search_guard(dir.path(), &config, input)
}

fn denied_reason(outcome: Option<HookOutcome>) -> String {
    let outcome = outcome.expect("expected a denial");
    assert_eq!(outcome.action, PermissionAction::Deny);
    outcome.reason.expect("reason")
}

#[test]
fn the_first_grep_is_denied_with_graph_and_docs_results_then_the_repeat_goes_through() {
    let dir = repo();
    let reason = denied_reason(guard(&dir, &grep("format|invoices")));
    assert!(reason.contains("## graph ("), "{reason}");
    assert!(reason.contains("symbol formatDate  src/format.ts:1"), "{reason}");
    assert!(reason.contains("## docs (1)"), "{reason}");
    assert!(reason.contains(CONCEPT_PATH), "{reason}");
    assert!(reason.contains("code not searched"), "{reason}");
    assert!(reason.ends_with(HINT), "{reason}");
    assert!(reason.len() <= 600 * 4 + HINT.len() + 200, "capped near 600 tokens: {}", reason.len());

    assert!(guard(&dir, &grep("format|invoices")).is_none(), "repeat must pass");
    assert!(guard(&dir, &grep("somethingelse")).is_none(), "the turn stays consulted");
}

#[test]
fn consulting_the_graph_or_docs_first_lets_the_search_through() {
    let dir = repo();
    let consults = [
        call(None, "mcp__anvil__graph_query", json!({ "term": "format" })),
        call(None, "mcp__anvil__graph_read_symbol", json!({ "symbol": "formatDate" })),
        call(None, "mcp__anvil__search", json!({ "query": "format" })),
        call(None, "mcp__anvil__context_pack", json!({ "targets": ["src/format.ts"] })),
        call(None, "mcp__anvil__code_exec", json!({ "script": "return 1" })),
        call(None, "mcp__plugin_anvil_anvil__graph_outline", json!({})),
        call(None, "Read", json!({ "file_path": dir.path().join(CONCEPT_PATH) })),
        shell("anvil search \"format\""),
        shell("anvil --root . graph query format"),
        shell("anvil docs search invoices"),
        shell("anvil context pack src/format.ts"),
        shell("cat .harness/docs/shop/invoicing/overview.md"),
        call(None, "Grep", json!({ "pattern": "invoices", "path": ".harness/docs" })),
    ];
    for consult in &consults {
        clear_turn(dir.path());
        assert!(guard(&dir, consult).is_none(), "{consult:?}");
        assert!(guard(&dir, &grep("formatDate")).is_none(), "grep after {consult:?}");
    }
    clear_turn(dir.path());
    assert!(guard(&dir, &call(None, "mcp__anvil__mem_search", json!({ "query": "x" }))).is_none());
    denied_reason(guard(&dir, &grep("formatDate")));
}

#[test]
fn shell_searches_of_the_repo_are_guarded_but_filters_and_outside_paths_are_not() {
    let dir = repo();
    denied_reason(guard(&dir, &shell("rg formatDate src")));

    for allowed in [
        "cargo test | grep formatDate",
        "git log --oneline | rg format",
        "rg formatDate /tmp/x",
        "grep -rn formatDate ~/notes",
        "ls src",
        "cargo build",
        "echo formatDate",
    ] {
        clear_turn(dir.path());
        assert!(guard(&dir, &shell(allowed)).is_none(), "{allowed}");
        denied_reason(guard(&dir, &shell("rg formatDate src")));
        assert!(
            guard(&dir, &shell(allowed)).is_none(),
            "{allowed} must never mark nor block the turn"
        );
    }
}

#[test]
fn every_search_program_is_recognised() {
    let dir = repo();
    for cmd in [
        "rg -n formatDate src",
        "rg -g '*.ts' -C 2 formatDate",
        "grep -rn formatDate src",
        "egrep -r 'format(Date|Money)' .",
        "fgrep -r formatDate .",
        "grep -e formatDate -r src",
        "git grep -n formatDate",
        "git -C . grep formatDate -- src",
        "ag formatDate",
        "ack formatDate",
        "tgrep formatDate",
        "anvil grep formatDate src",
        "anvil --root . grep -F formatDate",
        "findstr /s /i formatDate src\\*.ts",
        "findstr /c:\"format date\" src\\format.ts",
        "Select-String -Pattern formatDate -Path src\\*.ts",
        "sls formatDate src",
        "bash -c 'rg formatDate src'",
        "anvil run -- rg formatDate",
        "rg formatDate src | head -5",
    ] {
        clear_turn(dir.path());
        let reason = denied_reason(guard(&dir, &shell(cmd)));
        assert!(reason.contains("formatDate"), "{cmd}: {reason}");
    }
}

#[test]
fn search_tools_of_every_adapter_are_recognised() {
    let dir = repo();
    let searches = [
        call(None, "Grep", json!({ "pattern": "formatDate", "path": "src" })),
        call(None, "grep_search", json!({ "Query": "formatDate", "SearchPath": "src" })),
        call(None, "mcp__anvil__grep", json!({ "pattern": "formatDate", "paths": ["src"] })),
        call(None, "Glob", json!({ "pattern": "**/format*.ts" })),
        call(None, "find_by_name", json!({ "Pattern": "formatDate*", "SearchDirectory": "src" })),
    ];
    for search in &searches {
        clear_turn(dir.path());
        denied_reason(guard(&dir, search));
    }
    let ignored = [
        call(None, "Grep", json!({ "pattern": "formatDate", "path": "/definitely/outside" })),
        call(
            None,
            "mcp__anvil__grep",
            json!({ "pattern": "formatDate", "paths": ["/definitely/outside"] }),
        ),
        call(None, "mcp__other__grep", json!({ "pattern": "formatDate" })),
        call(None, "Glob", json!({ "pattern": "**/*.rs" })),
        call(None, "Grep", json!({ "pattern": "" })),
        call(None, "Grep", json!({ "pattern": ".*" })),
        call(None, "Grep", json!({ "notpattern": 1 })),
        call(None, "Edit", json!({ "file_path": "src/format.ts" })),
    ];
    for other in &ignored {
        clear_turn(dir.path());
        assert!(guard(&dir, other).is_none(), "{other:?}");
        denied_reason(guard(&dir, &grep("formatDate")));
    }
    let mut broken = grep("formatDate");
    broken.tool_input = None;
    clear_turn(dir.path());
    assert!(guard(&dir, &broken).is_none());
}

#[test]
fn each_agent_gets_its_own_first_search() {
    let dir = repo();
    let main = grep("formatDate");
    let sub = |id: &str| call(Some(id), "Grep", json!({ "pattern": "formatDate" }));

    denied_reason(guard(&dir, &main));
    assert!(guard(&dir, &main).is_none());
    denied_reason(guard(&dir, &sub("agent-1")));
    assert!(guard(&dir, &sub("agent-1")).is_none());
    denied_reason(guard(&dir, &sub("agent-2")));
    assert!(guard(&dir, &main).is_none());

    clear_turn(dir.path());
    let consult = call(Some("agent-1"), "mcp__anvil__graph_query", json!({ "term": "x" }));
    assert!(guard(&dir, &consult).is_none());
    assert!(guard(&dir, &sub("agent-1")).is_none(), "the subagent that consulted may search");
    denied_reason(guard(&dir, &main));
}

#[test]
fn flags_are_cleared_when_a_new_turn_starts() {
    let dir = repo();
    denied_reason(guard(&dir, &grep("formatDate")));
    assert!(guard(&dir, &grep("formatDate")).is_none());
    clear_turn(dir.path());
    denied_reason(guard(&dir, &grep("formatDate")));
}

#[test]
fn advise_mode_never_blocks_and_queues_one_line() {
    let dir = repo();
    assert!(guard_in(&dir, SearchCascadeMode::Advise, &grep("formatDate")).is_none());
    let queued = inbox::load_inbox(dir.path()).unwrap();
    assert_eq!(queued.len(), 1, "{queued:?}");
    let note = &queued[0].content;
    assert!(!note.contains('\n'), "{note}");
    assert!(note.starts_with("Search cascade: ") && note.contains("formatDate"), "{note}");
    assert!(note.contains("src/format.ts:1"), "{note}");

    assert!(guard_in(&dir, SearchCascadeMode::Advise, &grep("formatDate")).is_none());
    assert_eq!(inbox::load_inbox(dir.path()).unwrap().len(), 1, "advised once per turn");
}

#[test]
fn off_mode_and_a_missing_cascade_target_allow_everything() {
    let dir = repo();
    assert!(guard_in(&dir, SearchCascadeMode::Off, &grep("formatDate")).is_none());
    assert!(!dir.path().join(".harness/.state/search-cascade.json").exists(), "off keeps no state");

    let bare = TempDir::new().unwrap();
    fs::create_dir_all(bare.path().join(".harness")).unwrap();
    write(bare.path(), "src/format.ts", FORMATTERS);
    assert!(
        guard(&bare, &grep("formatDate")).is_none(),
        "no graph and no docs: nothing to cascade into"
    );

    let no_harness = TempDir::new().unwrap();
    assert!(guard(&no_harness, &grep("formatDate")).is_none());
}

#[test]
fn a_search_with_no_graph_or_docs_hit_passes_and_counts_as_consulted() {
    let dir = repo();
    assert!(guard(&dir, &grep("zzzunknownzzz")).is_none());
    assert!(guard(&dir, &grep("formatDate")).is_none(), "the empty answer already consulted");
}

#[test]
fn docs_alone_are_enough_to_cascade_into() {
    let dir = TempDir::new().unwrap();
    write(dir.path(), CONCEPT_PATH, CONCEPT);
    let reason = denied_reason(guard(&dir, &grep("invoices")));
    assert!(reason.contains("## graph: no code graph"), "{reason}");
    assert!(reason.contains(CONCEPT_PATH), "{reason}");
}

#[test]
fn anvil_search_is_readable_for_both_orchestrator_roles() {
    for ok in ["anvil search \"security guard\"", "anvil --root . search foo --code --json"] {
        assert_eq!(why_not_read_only(ok), None, "orchestrator: {ok}");
        assert_eq!(why_not_executor(ok), None, "executor: {ok}");
    }
}
