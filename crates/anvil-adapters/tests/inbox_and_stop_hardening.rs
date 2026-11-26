use std::sync::{Arc, Barrier};
use std::thread;

use serde_json::json;
use tempfile::tempdir;

use anvil_adapters::antigravity;
use anvil_adapters::claude_code;
use anvil_adapters::contract::{HookEvent, HookOutcome, InboxMessage, MessageLevel};
use anvil_adapters::inbox;
use anvil_adapters::templates;

#[test]
fn locked_inbox_preserves_all_concurrent_pushes_and_drains_cleanly() {
    let dir = tempdir().expect("tempdir");
    let root = Arc::new(dir.path().to_path_buf());
    let workers = 8;
    let per_worker = 4;
    let barrier = Arc::new(Barrier::new(workers));

    let mut handles = Vec::with_capacity(workers);
    for w in 0..workers {
        let root = Arc::clone(&root);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            barrier.wait();
            for i in 0..per_worker {
                let msg = InboxMessage::new(
                    format!("worker-{w}"),
                    MessageLevel::Warning,
                    format!("diag-{w}-{i}"),
                    Some(format!("src/file_{w}.rs")),
                );
                inbox::push_messages(&root, &[msg]).expect("locked push");
            }
        }));
    }
    for h in handles {
        h.join().expect("worker joined");
    }

    let drained = inbox::drain_inbox(&root).expect("locked drain");
    assert_eq!(drained.len(), workers * per_worker);
    assert!(inbox::load_inbox(&root).expect("load after drain").is_empty());

    inbox::push_messages(
        &root,
        &[InboxMessage::new("test", MessageLevel::Info, "msg", None)],
    )
    .expect("re-push");
    inbox::clear_inbox(&root).expect("locked clear");
    assert!(inbox::load_inbox(&root).expect("load after clear").is_empty());
}

#[test]
fn antigravity_render_stop_preserves_messages_on_block_and_allow() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();

    let blocking = HookOutcome {
        block_stop: true,
        reason: Some("Tier 1 failed".to_owned()),
        messages: vec![InboxMessage::new(
            "tier1",
            MessageLevel::Error,
            "2 type errors in src/app.ts",
            Some("src/app.ts".to_owned()),
        )],
        ..HookOutcome::allow()
    };
    let out_block = antigravity::render_output(root, HookEvent::Stop, &blocking)
        .expect("render blocking stop");
    assert_eq!(out_block["decision"], "continue");
    let reason = out_block["reason"].as_str().expect("blocking reason");
    assert!(reason.contains("Tier 1 failed"));
    assert!(reason.contains("Anvil · src/app.ts: 2 type errors in src/app.ts"));

    let blocking_no_reason = HookOutcome {
        block_stop: true,
        reason: None,
        messages: vec![InboxMessage::new(
            "tasks",
            MessageLevel::Warning,
            "Active task T-1 is still in progress",
            None,
        )],
        ..HookOutcome::allow()
    };
    let out_block_msg = antigravity::render_output(root, HookEvent::Stop, &blocking_no_reason)
        .expect("render blocking stop without reason");
    assert_eq!(out_block_msg["decision"], "continue");
    assert!(
        out_block_msg["reason"]
            .as_str()
            .expect("reason")
            .contains("Active task T-1 is still in progress")
    );

    let allowing_with_msgs = HookOutcome::with_messages(vec![InboxMessage::new(
        "ratchet",
        MessageLevel::Info,
        "Candidate rule logged",
        None,
    )]);
    let out_allow = antigravity::render_output(root, HookEvent::Stop, &allowing_with_msgs)
        .expect("render allowing stop");
    assert_eq!(out_allow, json!({ "decision": "allow" }));

    let pre_inv = antigravity::render_output(root, HookEvent::PreInvocation, &HookOutcome::allow())
        .expect("render PreInvocation");
    let injected = pre_inv["injectSteps"][0]["systemMessage"]["systemMessage"]
        .as_str()
        .expect("injected systemMessage");
    assert!(injected.contains("[info][ratchet]: Candidate rule logged"));
}

#[test]
fn claude_code_render_stop_surfaces_messages_on_block_and_allow() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();

    let blocking = HookOutcome {
        block_stop: true,
        reason: Some("Verification failed".to_owned()),
        messages: vec![InboxMessage::new(
            "tier0",
            MessageLevel::Error,
            "Disallowed @ts-ignore",
            Some("src/index.ts".to_owned()),
        )],
        ..HookOutcome::allow()
    };
    let out_block = claude_code::render_output(root, HookEvent::Stop, &blocking)
        .expect("render claude blocking stop");
    assert_eq!(out_block["decision"], "block");
    let reason = out_block["reason"].as_str().expect("reason");
    assert!(reason.contains("Verification failed"));
    assert!(reason.contains("Anvil · src/index.ts: Disallowed @ts-ignore"));

    let allowing_with_msgs = HookOutcome::with_messages(vec![InboxMessage::new(
        "anvil-tasks",
        MessageLevel::Info,
        "Reminder: update task T-42",
        None,
    )]);
    let out_allow = claude_code::render_output(root, HookEvent::Stop, &allowing_with_msgs)
        .expect("render claude allowing stop");
    assert!(out_allow.get("decision").is_none());
    assert!(
        out_allow["systemMessage"]
            .as_str()
            .expect("systemMessage")
            .contains("Anvil · Reminder: update task T-42")
    );
}

#[test]
fn adapter_templates_stay_compact_and_accurate() {
    for doc in [templates::AGENTS_MD, templates::CLAUDE_MD] {
        assert!(doc.lines().count() <= 32);
        assert!(doc.contains("anvil ratchet log|list|review|test|promote"));
        assert!(doc.contains("anvil qa run|onboard|eval-web|scan-mobile"));
        assert!(doc.contains("Tier 1"));
    }
}

#[test]
fn inbox_deduplicates_identical_messages_and_caps_at_32() {
    let dir = tempdir().expect("tempdir");
    let root = dir.path();

    let dup_a = InboxMessage::new(
        "tier1",
        MessageLevel::Warning,
        "unused import",
        Some("src/lib.rs".to_owned()),
    );
    let dup_b = InboxMessage::new(
        "tier1",
        MessageLevel::Error,
        "unused import",
        Some("src/lib.rs".to_owned()),
    );
    inbox::push_messages(root, &[dup_a.clone(), dup_b.clone()]).expect("batch dedup push");
    inbox::push_messages(root, &[dup_a]).expect("repeat dedup push");

    let deduped = inbox::drain_inbox(root).expect("drain deduped");
    assert_eq!(deduped.len(), 1);
    assert_eq!(deduped[0].content, "unused import");

    let distinct: Vec<InboxMessage> = (0..40)
        .map(|i| {
            InboxMessage::new(
                "tier1",
                MessageLevel::Info,
                format!("diag-{i}"),
                Some("src/lib.rs".to_owned()),
            )
        })
        .collect();
    inbox::push_messages(root, &distinct).expect("push 40 distinct");

    let capped = inbox::drain_inbox(root).expect("drain capped");
    assert_eq!(capped.len(), 32);
    let contents: Vec<&str> = capped.iter().map(|m| m.content.as_str()).collect();
    let expected: Vec<String> = (8..40).map(|i| format!("diag-{i}")).collect();
    let expected_refs: Vec<&str> = expected.iter().map(String::as_str).collect();
    assert_eq!(contents, expected_refs);
}
