use std::fs;

use serde_json::json;
use tempfile::tempdir;

use anvil_adapters::antigravity;
use anvil_adapters::claude_code;
use anvil_adapters::contract::{
    AdapterKind, HookEvent, HookOutcome, InboxMessage, MessageLevel,
};
use anvil_adapters::inbox;

#[test]
fn antigravity_session_start_and_stop_roundtrip() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();

    let session_in = r#"{"hookEventName":"SessionStart","cwd":"/repo","sessionId":"sess-1"}"#;
    let parsed = antigravity::parse_input(session_in, None).expect("parse SessionStart");
    assert_eq!(parsed.adapter, AdapterKind::Antigravity);
    assert_eq!(parsed.event, HookEvent::SessionStart);
    assert_eq!(parsed.cwd.as_deref(), Some("/repo"));
    assert_eq!(parsed.session_id.as_deref(), Some("sess-1"));

    let out = antigravity::render_output(
        root,
        HookEvent::SessionStart,
        &HookOutcome::with_messages(vec![InboxMessage::new(
            "anvil-tasks",
            MessageLevel::Info,
            "Active task T-1",
            None,
        )]),
    )
    .expect("render SessionStart");
    let msg = out["injectSteps"][0]["systemMessage"]["systemMessage"]
        .as_str()
        .expect("systemMessage");
    assert!(msg.contains("Active task T-1"));

    assert_eq!(
        antigravity::render_output(
            root,
            HookEvent::Stop,
            &HookOutcome::block_stop("Tier 1 failed"),
        )
        .expect("stop continue"),
        json!({ "decision": "continue", "reason": "Anvil · Tier 1 failed" })
    );
    assert_eq!(
        antigravity::render_output(root, HookEvent::Stop, &HookOutcome::allow())
            .expect("stop allow"),
        json!({ "decision": "allow" })
    );
}

#[test]
fn antigravity_pre_tool_use_decisions_and_overwrite() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();

    let pre_in = r#"{
        "hookEventName": "PreToolUse",
        "cwd": "/repo",
        "toolCall": { "toolName": "Bash", "toolInput": { "command": "vitest run" } }
    }"#;
    let parsed = antigravity::parse_input(pre_in, None).expect("parse PreToolUse");
    assert_eq!(parsed.tool_name.as_deref(), Some("Bash"));
    assert_eq!(parsed.tool_input, Some(json!({ "command": "vitest run" })));

    assert_eq!(
        antigravity::render_output(root, HookEvent::PreToolUse, &HookOutcome::allow())
            .expect("allow"),
        json!({ "decision": "allow" })
    );
    assert_eq!(
        antigravity::render_output(
            root,
            HookEvent::PreToolUse,
            &HookOutcome::deny("circuit open"),
        )
        .expect("deny"),
        json!({ "decision": "deny", "reason": "circuit open" })
    );
    assert_eq!(
        antigravity::render_output(root, HookEvent::PreToolUse, &HookOutcome::ask("confirm rm"))
            .expect("ask"),
        json!({ "decision": "ask", "reason": "confirm rm" })
    );
    assert_eq!(
        antigravity::render_output(
            root,
            HookEvent::PreToolUse,
            &HookOutcome::overwrite(
                json!({ "command": "anvil run -- vitest run" }),
                Some("wrapped with anvil run".to_owned()),
            ),
        )
        .expect("overwrite"),
        json!({
            "decision": "allow",
            "reason": "wrapped with anvil run",
            "overwrite": { "command": "anvil run -- vitest run" }
        })
    );
}

#[test]
fn antigravity_post_tool_use_queues_inbox_and_pre_invocation_drains_it() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();

    let post_in = r#"{
        "hookEventName": "PostToolUse",
        "cwd": "/repo",
        "toolCall": {
            "toolName": "Edit",
            "toolInput": { "path": "src/app.ts" },
            "toolOutput": { "ok": true }
        }
    }"#;
    let parsed_post = antigravity::parse_input(post_in, None).expect("parse PostToolUse");
    assert_eq!(parsed_post.tool_output, Some(json!({ "ok": true })));

    let post_out = antigravity::render_output(
        root,
        HookEvent::PostToolUse,
        &HookOutcome::with_messages(vec![InboxMessage::new(
            "tier0-ast",
            MessageLevel::Error,
            "Disallowed @ts-ignore",
            Some("src/app.ts".to_owned()),
        )]),
    )
    .expect("render PostToolUse");
    assert_eq!(post_out, json!({}));
    assert_eq!(inbox::load_inbox(root).expect("load inbox").len(), 1);

    let pre_inv_in = r#"{"hookEventName":"PreInvocation","cwd":"/repo","prompt":"Continue"}"#;
    let parsed_inv = antigravity::parse_input(pre_inv_in, None).expect("parse PreInvocation");
    assert_eq!(parsed_inv.prompt.as_deref(), Some("Continue"));

    let inv_out = antigravity::render_output(root, HookEvent::PreInvocation, &HookOutcome::allow())
        .expect("render PreInvocation");
    let injected = inv_out["injectSteps"][0]["systemMessage"]["systemMessage"]
        .as_str()
        .expect("injected string");
    assert!(injected.contains("[error][tier0-ast] (src/app.ts): Disallowed @ts-ignore"));
    assert!(inbox::load_inbox(root).expect("drained").is_empty());

    let inv_empty =
        antigravity::render_output(root, HookEvent::PreInvocation, &HookOutcome::allow())
            .expect("empty PreInvocation");
    assert_eq!(inv_empty, json!({}));
    let raw_inbox = fs::read_to_string(inbox::inbox_path(root)).expect("read inbox.json");
    assert!(raw_inbox.contains("\"messages\":[]"));
}

#[test]
fn claude_code_session_start_and_stop_roundtrip() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();

    let session_in = r#"{"hook_event_name":"SessionStart","cwd":"/repo","session_id":"claude-1"}"#;
    let parsed_session = claude_code::parse_input(session_in, None).expect("parse SessionStart");
    assert_eq!(parsed_session.adapter, AdapterKind::ClaudeCode);
    assert_eq!(parsed_session.session_id.as_deref(), Some("claude-1"));

    let session_out = claude_code::render_output(
        root,
        HookEvent::SessionStart,
        &HookOutcome::with_messages(vec![InboxMessage::new(
            "anvil-mem",
            MessageLevel::Info,
            "Loaded 2 decisions",
            None,
        )]),
    )
    .expect("render SessionStart");
    assert_eq!(session_out["hookSpecificOutput"]["hookEventName"], "SessionStart");
    assert!(
        session_out["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("additionalContext")
            .contains("Loaded 2 decisions")
    );

    assert_eq!(
        claude_code::render_output(root, HookEvent::Stop, &HookOutcome::block_stop("tsc failed"))
            .expect("stop block"),
        json!({ "decision": "block", "reason": "Anvil · tsc failed" })
    );
    assert_eq!(
        claude_code::render_output(root, HookEvent::Stop, &HookOutcome::allow())
            .expect("stop allow"),
        json!({})
    );
}

#[test]
fn claude_code_pre_tool_use_decisions_and_updated_input() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();

    let pre_in = r#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"pytest"}}"#;
    let parsed_pre = claude_code::parse_input(pre_in, None).expect("parse PreToolUse");
    assert_eq!(parsed_pre.tool_name.as_deref(), Some("Bash"));

    assert_eq!(
        claude_code::render_output(
            root,
            HookEvent::PreToolUse,
            &HookOutcome::deny("blocked by circuit breaker"),
        )
        .expect("deny"),
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "blocked by circuit breaker"
            }
        })
    );
    assert_eq!(
        claude_code::render_output(
            root,
            HookEvent::PreToolUse,
            &HookOutcome::overwrite(
                json!({ "command": "anvil run -- pytest" }),
                Some("use compressed runner".to_owned()),
            ),
        )
        .expect("overwrite"),
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "updatedInput": { "command": "anvil run -- pytest" }
            }
        }),
        "rewritten input must still go through the user's permission flow"
    );
    assert_eq!(
        claude_code::render_output(root, HookEvent::PreToolUse, &HookOutcome::ask("confirm rm"))
            .expect("ask"),
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "ask",
                "permissionDecisionReason": "confirm rm"
            }
        })
    );
    assert_eq!(
        claude_code::render_output(root, HookEvent::PreToolUse, &HookOutcome::allow())
            .expect("allow"),
        json!({}),
        "an unguarded call must stay silent instead of auto-approving the tool"
    );
}

#[test]
fn claude_code_post_tool_use_returns_additional_context_and_clears_inbox() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();

    inbox::push_messages(
        root,
        &[InboxMessage::new(
            "stale",
            MessageLevel::Warning,
            "Prior pending notice",
            None,
        )],
    )
    .expect("seed inbox");

    let post_in = r#"{
        "hook_event_name": "PostToolUse",
        "tool_name": "Write",
        "tool_input": { "file_path": "src/main.ts" },
        "tool_response": { "success": true }
    }"#;
    let parsed_post = claude_code::parse_input(post_in, None).expect("parse PostToolUse");
    assert_eq!(parsed_post.tool_output, Some(json!({ "success": true })));

    let post_out = claude_code::render_output(
        root,
        HookEvent::PostToolUse,
        &HookOutcome::with_messages(vec![InboxMessage::new(
            "tier0-ast",
            MessageLevel::Warning,
            "Avoid narrative comment",
            Some("src/main.ts".to_owned()),
        )]),
    )
    .expect("render PostToolUse");

    let ctx = post_out["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .expect("additionalContext");
    assert!(ctx.contains("Prior pending notice"));
    assert!(ctx.contains("Avoid narrative comment"));
    assert!(inbox::load_inbox(root).expect("inbox cleared").is_empty());
}

#[test]
fn user_prompt_and_last_user_input_roundtrip() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();

    let agy_in = r#"{"hookEventName":"PreInvocation","cwd":"/repo","lastUserInput":"Add dark mode toggle"}"#;
    let parsed_agy = antigravity::parse_input(agy_in, None).expect("parse agy PreInvocation");
    assert_eq!(parsed_agy.user_prompt.as_deref(), Some("Add dark mode toggle"));

    let claude_in = r#"{"hook_event_name":"UserPromptSubmit","cwd":"/repo","prompt":"Add user profile page"}"#;
    let parsed_claude = claude_code::parse_input(claude_in, None).expect("parse UserPromptSubmit");
    assert_eq!(parsed_claude.event, HookEvent::UserPromptSubmit);
    assert_eq!(parsed_claude.user_prompt.as_deref(), Some("Add user profile page"));

    let rendered = claude_code::render_output(
        root,
        HookEvent::UserPromptSubmit,
        &HookOutcome::with_messages(vec![InboxMessage::new(
            "anvil-tasks",
            MessageLevel::Info,
            "Auto-registered task T-1",
            None,
        )]),
    )
    .expect("render UserPromptSubmit");
    assert_eq!(rendered["hookSpecificOutput"]["hookEventName"], "UserPromptSubmit");
    assert!(
        rendered["hookSpecificOutput"]["additionalContext"]
            .as_str()
            .expect("additionalContext")
            .contains("Auto-registered task T-1")
    );
    assert_eq!(
        claude_code::render_output(root, HookEvent::UserPromptSubmit, &HookOutcome::allow())
            .expect("empty UserPromptSubmit"),
        json!({})
    );
}
