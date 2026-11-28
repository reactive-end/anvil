//! `anvil prompt improve`, `anvil prompt assist`, and the prompt hook end to end in a temporary
//! repository that has a code graph, one API scenario, and one web flow.

use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};
use tempfile::TempDir;

const ORDER_TS: &str = "export class Order {\n  total(): number {\n    return 0;\n  }\n}\n\nexport function applyDiscount(order: Order, percent: number): number {\n  return order.total() * (1 - percent / 100);\n}\n";
const SCENARIO: &str = r#"{ "name": "health", "steps": [ { "http": { "method": "GET", "path": "/health" }, "expect": { "status": 200 } } ] }"#;
const FLOW: &str = r#"{ "name": "home", "url": "http://localhost:5180/", "steps": [ { "goto": "http://localhost:5180/" } ] }"#;

fn anvil(cwd: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_anvil"))
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .env("ANVIL_NO_AUTO_DAEMON", "1")
        .env("ANVIL_LANG", "en")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn anvil");
    if let Some(mut pipe) = child.stdin.take()
        && let Some(text) = stdin
    {
        pipe.write_all(text.as_bytes()).expect("stdin");
    }
    child.wait_with_output().expect("wait")
}

fn ok(cwd: &Path, args: &[&str]) -> String {
    let out = anvil(cwd, args, None);
    assert!(
        out.status.success(),
        "anvil {args:?} failed: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn write(root: &Path, rel: &str, text: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    fs::write(path, text).expect("write");
}

fn repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let root = dir.path();
    write(root, "src/orders/order.ts", ORDER_TS);
    write(root, ".harness/qa/api/health.json", SCENARIO);
    write(root, ".harness/qa/flows/home.json", FLOW);
    ok(root, &["graph", "build"]);
    dir
}

/// Runs the Claude Code `UserPromptSubmit` hook for `prompt` in `session`.
fn prompt_hook(root: &Path, session: &str, prompt: &str) -> Value {
    let payload = json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": session,
        "cwd": root.to_string_lossy(),
        "prompt": prompt,
    });
    let out = anvil(
        root,
        &["hook", "user-prompt-submit", "--adapter", "claude"],
        Some(&payload.to_string()),
    );
    assert!(out.status.success(), "hook failed: {}", String::from_utf8_lossy(&out.stderr));
    serde_json::from_slice(&out.stdout).expect("hook JSON")
}

fn context_of(out: &Value) -> String {
    out["hookSpecificOutput"]["additionalContext"].as_str().unwrap_or_default().to_owned()
}

#[test]
fn the_hook_adds_a_context_line_naming_the_symbol_and_its_location() {
    let dir = repo();
    let out = prompt_hook(dir.path(), "s1", "make applyDiscount round the total to cents");
    let context = context_of(&out);
    assert!(context.contains("[prompt-context]"), "{out}");
    assert!(
        context.contains(
            "Likely relevant (verify before relying): code: applyDiscount (src/orders/order.ts:7)"
        ),
        "{context}"
    );
    assert!(out.get("systemMessage").is_none(), "context mode never talks to the user: {out}");
}

#[test]
fn slash_commands_injected_text_and_short_prompts_get_nothing() {
    let dir = repo();
    for prompt in [
        "/compact",
        "<task-notification>applyDiscount finished running the whole build</task-notification>",
        "yes applyDiscount please",
    ] {
        let out = prompt_hook(dir.path(), "s2", prompt);
        assert!(!context_of(&out).contains("Likely relevant"), "{prompt}: {out}");
    }
}

#[test]
fn an_item_is_not_repeated_in_the_next_prompts_of_the_same_session() {
    let dir = repo();
    let root = dir.path();
    let first = context_of(&prompt_hook(root, "s3", "make applyDiscount round the total to cents"));
    assert!(first.contains("applyDiscount"), "{first}");
    let second =
        context_of(&prompt_hook(root, "s3", "also make applyDiscount reject negative percent"));
    assert!(!second.contains("Likely relevant"), "repeated within 3 prompts: {second}");
    let other_session =
        context_of(&prompt_hook(root, "s4", "also make applyDiscount reject negative percent"));
    assert!(other_session.contains("applyDiscount"), "{other_session}");
}

#[test]
fn nothing_relevant_means_no_block_at_all() {
    let dir = repo();
    let out = prompt_hook(dir.path(), "s5", "please translate the welcome banner into klingon");
    assert_eq!(out, json!({}), "{out}");
}

#[test]
fn the_block_never_exceeds_the_configured_token_cap() {
    let dir = repo();
    let root = dir.path();
    write(root, ".harness/config.json", r#"{ "prompt_assist": { "max_tokens": 40 } }"#);
    let context =
        context_of(&prompt_hook(root, "s6", "make applyDiscount and Order agree on the total"));
    assert!(context.contains("applyDiscount") || context.contains("Order"), "{context}");
    assert!(context.chars().count() <= 40 * 4, "{} chars: {context}", context.chars().count());
}

#[test]
fn coach_mode_adds_a_note_for_the_agent_and_a_tip_for_the_user_once_per_gap() {
    let dir = repo();
    let root = dir.path();
    assert!(ok(root, &["prompt", "assist", "coach"]).contains("set to coach"));
    let out = prompt_hook(root, "s7", "arregla todo lo del login");
    let context = context_of(&out);
    assert!(context.contains("[prompt-coach]"), "{out}");
    assert!(
        context.contains("Coach: this request has no file, function, or doc it points at"),
        "{context}"
    );
    assert!(
        context.contains("`anvil check`")
            && context.contains("`anvil qa api`")
            && context.contains("`anvil qa run --all-flows`"),
        "{context}"
    );
    assert!(context.contains("Do not edit until they answer."), "{context}");
    let tip = out["systemMessage"].as_str().expect("user tip");
    assert!(tip.starts_with("Anvil · Tip: name the file, function, or screen"), "{tip}");

    let mut tips = vec![tip.to_owned()];
    for prompt in [
        "arregla todo lo del carrito",
        "fix everything about the banner",
        "tidy up the whole footer now",
        "rename the sidebar title again",
    ] {
        if let Some(t) = prompt_hook(root, "s7", prompt)["systemMessage"].as_str() {
            tips.push(t.to_owned());
        }
    }
    let mut unique = tips.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), tips.len(), "a tip was shown twice: {tips:?}");
}

#[test]
fn coach_stays_quiet_for_a_precise_request() {
    let dir = repo();
    let root = dir.path();
    ok(root, &["prompt", "assist", "coach"]);
    let out = prompt_hook(root, "s8", "applyDiscount should return 0 for percent 100, add a test");
    assert!(!context_of(&out).contains("Coach:"), "{out}");
    assert!(out.get("systemMessage").is_none(), "{out}");
}

#[test]
fn assist_off_silences_the_hook_and_the_mode_is_saved_without_defaults() {
    let dir = repo();
    let root = dir.path();
    assert_eq!(
        ok(root, &["prompt", "assist"]).trim(),
        "prompt assist is context (block cap 450 tokens, skips prompts under 4 words)"
    );
    ok(root, &["prompt", "assist", "off"]);
    let saved = fs::read_to_string(root.join(".harness/config.json")).expect("config");
    assert!(saved.contains("\"prompt_assist\"") && saved.contains("\"off\""), "{saved}");
    assert_eq!(prompt_hook(root, "s9", "make applyDiscount round the total to cents"), json!({}));
    ok(root, &["prompt", "assist", "context"]);
    let saved = fs::read_to_string(root.join(".harness/config.json")).expect("config");
    assert!(!saved.contains("prompt_assist"), "defaults are not written: {saved}");
}

#[test]
fn improve_prints_every_section_built_from_the_repository() {
    let dir = repo();
    let root = dir.path();
    ok(root, &["architecture", "set", "hexagonal"]);
    let md = ok(root, &["prompt", "improve", "round the applyDiscount total"]);
    let titles = [
        "## Goal",
        "## Context",
        "## Constraints",
        "## Acceptance criteria",
        "## Verification",
        "## Open questions",
    ];
    let at: Vec<usize> = titles.iter().map(|t| md.find(t).unwrap_or(usize::MAX)).collect();
    assert!(at.windows(2).all(|w| w[0] < w[1]), "{md}");
    assert!(md.starts_with("## Goal\nround the applyDiscount total\n"), "{md}");
    assert!(md.contains("- code: `applyDiscount` (function src/orders/order.ts:7)"), "{md}");
    assert!(md.contains("Architecture: hexagonal"), "{md}");
    assert!(md.contains("`anvil-allow(<rule>): <reason>`"), "{md}");
    assert!(md.contains("- [ ] <describe what done looks like>"), "{md}");
    assert!(md.contains("- `anvil qa api`: 1 API scenario(s): health"), "{md}");
    assert!(md.contains("- `anvil qa run --flow home`"), "{md}");
    assert!(md.contains("- What should be true when this is done"), "{md}");
}

#[test]
fn improve_reads_stdin_and_prints_json() {
    let dir = repo();
    let root = dir.path();
    let out = anvil(
        root,
        &["prompt", "improve", "--json", "-"],
        Some("fix everything about the login\n"),
    );
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let data: Value = serde_json::from_slice(&out.stdout).expect("json");
    assert_eq!(data["goal"], "fix everything about the login");
    let gaps: Vec<&str> =
        data["gaps"].as_array().expect("gaps").iter().filter_map(Value::as_str).collect();
    assert!(gaps.contains(&"no_target") && gaps.contains(&"broad_scope"), "{data}");
    assert!(
        data["verification"][0].as_str().expect("first").starts_with("`anvil check`"),
        "{data}"
    );
    assert!(data["open_questions"].as_array().is_some_and(|q| !q.is_empty()), "{data}");
}

#[test]
fn improve_without_text_fails_instead_of_inventing_a_prompt() {
    let dir = repo();
    let out = anvil(dir.path(), &["prompt", "improve"], Some(""));
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no prompt text"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}
