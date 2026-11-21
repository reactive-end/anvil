//! Integration tests for `anvil-memory`: FTS5, upserts, redaction, Git chunks, staleness, TTL.

use std::fs;
use std::time::Instant;

use anvil_core::traits::{FreshnessReport, FreshnessState, SymbolFreshnessProvider};
use anvil_memory::{MemoryEngine, MemoryKind, ObservationInput};
use tempfile::TempDir;

struct MockFreshnessProvider {
    changed_files: Vec<String>,
    missing_symbols: Vec<String>,
}

impl SymbolFreshnessProvider for MockFreshnessProvider {
    fn check_freshness(
        &self,
        related_files: &[String],
        related_symbols: &[String],
    ) -> anyhow::Result<FreshnessReport> {
        for sym in related_symbols {
            if self.missing_symbols.contains(sym) {
                return Ok(FreshnessReport {
                    state: FreshnessState::Stale,
                    reason: Some(format!("symbol removed: {sym}")),
                });
            }
        }
        for file in related_files {
            if self.changed_files.contains(file) {
                return Ok(FreshnessReport {
                    state: FreshnessState::NeedsVerification,
                    reason: Some(format!("file modified: {file}")),
                });
            }
        }
        Ok(FreshnessReport {
            state: FreshnessState::Fresh,
            reason: None,
        })
    }
}

fn sample_input(kind: MemoryKind, topic: &str, title: &str, what: &str) -> ObservationInput {
    let mut input = ObservationInput::new(
        kind,
        title,
        what,
        "Preserve deterministic architectural constraints",
        "crates/anvil-memory/src/store.rs",
        "FTS5 virtual table stays synchronized via triggers",
    );
    input.topic_key = Some(topic.to_owned());
    input
}

#[test]
fn fts5_bm25_progressive_disclosure_under_5ms() {
    let tmp = TempDir::new().expect("tempdir");
    let mut engine = MemoryEngine::open(tmp.path(), "alice").expect("open engine");

    for idx in 0..40 {
        let topic = format!("topic-{idx}");
        let title = if idx == 27 {
            "SQLite WAL FTS5 BM25 indexing strategy".to_owned()
        } else {
            format!("Unrelated subsystem note {idx}")
        };
        let what = if idx == 27 {
            "Use bundled rusqlite with FTS5 porter tokenizer for sub-millisecond BM25 search"
                .to_owned()
        } else {
            format!("Routine configuration record number {idx} for build cache")
        };
        engine
            .save(sample_input(MemoryKind::Decision, &topic, &title, &what))
            .expect("save obs");
    }

    let start = Instant::now();
    let hits = engine
        .search("rusqlite FTS5 BM25", Some(MemoryKind::Decision), 5)
        .expect("search");
    let elapsed = start.elapsed();

    assert!(!hits.is_empty(), "expected BM25 search hits");
    assert_eq!(hits[0].topic_key, "topic-27");
    assert!(
        elapsed.as_millis() < 5,
        "FTS5 search took {:?}, expected < 5ms",
        elapsed
    );

    let full = engine.get(&hits[0].id).expect("get").expect("obs exists");
    assert!(full.learned.contains("triggers"));
    assert_eq!(full.why, "Preserve deterministic architectural constraints");

    let tl = engine.timeline(&hits[0].id, 2, 2).expect("timeline");
    assert!(tl.iter().any(|e| e.id == hits[0].id && e.is_anchor));

    let ctx = engine.session_context(200).expect("session context");
    assert!(!ctx.is_empty());
}

#[test]
fn topic_key_upsert_updates_without_duplicates() {
    let tmp = TempDir::new().expect("tempdir");
    let mut engine = MemoryEngine::open(tmp.path(), "alice").expect("open engine");

    let first = engine
        .save(sample_input(
            MemoryKind::Decision,
            "auth/jwt-rotation",
            "JWT Rotation v1",
            "Rotate signing keys every 30 days",
        ))
        .expect("first save");

    let second = engine
        .save(sample_input(
            MemoryKind::Decision,
            "auth/jwt-rotation",
            "JWT Rotation v2",
            "Rotate signing keys every 7 days using KMS",
        ))
        .expect("second save");

    assert_eq!(first.id, second.id);
    assert_eq!(second.title, "JWT Rotation v2");

    let hits = engine.search("JWT Rotation", None, 10).expect("search");
    assert_eq!(hits.len(), 1, "upsert must not create duplicate rows");
    assert_eq!(hits[0].title, "JWT Rotation v2");
}

#[test]
fn redactor_strips_private_blocks_and_secrets_everywhere() {
    let tmp = TempDir::new().expect("tempdir");
    let mut engine = MemoryEngine::open(tmp.path(), "sec-tester").expect("open engine");

    let mut input = sample_input(
        MemoryKind::Gotcha,
        "env/secrets-gotcha",
        "CI Auth <private>super-secret-inline</private> Config",
        "Token ghp_AbCdEfGhIjKlMnOpQrStUvWxYz123456 and AKIA1234567890ABCDEF",
    );
    input.why = "Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0In0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c".to_owned();
    input.learned = "postgres://admin:UltraSecretPass99@db.internal:5432/app\n<private>\nTOP_SECRET_BLOCK\n</private>".to_owned();

    let saved = engine.save(input).expect("save redacted");

    let forbidden = [
        "super-secret-inline",
        "TOP_SECRET_BLOCK",
        "<private>",
        "ghp_AbCdEfGhIjKlMnOpQrStUvWxYz123456",
        "AKIA1234567890ABCDEF",
        "SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c",
        "UltraSecretPass99",
    ];

    for secret in forbidden {
        assert!(!saved.title.contains(secret), "leaked in title: {secret}");
        assert!(!saved.what.contains(secret), "leaked in what: {secret}");
        assert!(!saved.why.contains(secret), "leaked in why: {secret}");
        assert!(!saved.learned.contains(secret), "leaked in learned: {secret}");
    }

    let chunks_dir = tmp.path().join(".harness/memory/chunks");
    for entry in fs::read_dir(&chunks_dir).expect("read chunks dir") {
        let raw_chunk = fs::read_to_string(entry.expect("entry").path()).expect("read chunk");
        for secret in forbidden {
            assert!(!raw_chunk.contains(secret), "leaked in chunk file: {secret}");
        }
    }

    let db_obs = engine.get(&saved.id).expect("get").expect("exists");
    for secret in forbidden {
        assert!(!db_obs.what.contains(secret));
        assert!(!db_obs.why.contains(secret));
        assert!(!db_obs.learned.contains(secret));
    }
}

#[test]
fn multi_author_chunk_sync_is_conflict_free_and_idempotent() {
    let repo_a = TempDir::new().expect("repo_a");
    let repo_b = TempDir::new().expect("repo_b");
    let merged_repo = TempDir::new().expect("merged_repo");

    let mut eng_a = MemoryEngine::open(repo_a.path(), "alice").expect("alice");
    let mut eng_b = MemoryEngine::open(repo_b.path(), "bob").expect("bob");

    eng_a
        .save(sample_input(
            MemoryKind::Domain,
            "billing/tax-rounding",
            "Tax Rounding Rule",
            "Round VAT half-even at invoice line level",
        ))
        .expect("alice save");
    eng_b
        .save(sample_input(
            MemoryKind::Gotcha,
            "android/gradle-daemon",
            "Gradle OOM Gotcha",
            "Set org.gradle.jvmargs=-Xmx4g on CI runners",
        ))
        .expect("bob save");

    let merged_chunks = merged_repo.path().join(".harness/memory/chunks");
    fs::create_dir_all(&merged_chunks).expect("mkdir chunks");

    for dir in [
        repo_a.path().join(".harness/memory/chunks"),
        repo_b.path().join(".harness/memory/chunks"),
    ] {
        for entry in fs::read_dir(dir).expect("read dir") {
            let src = entry.expect("entry").path();
            let dst = merged_chunks.join(src.file_name().expect("filename"));
            fs::copy(src, dst).expect("copy chunk");
        }
    }

    let mut fresh_engine = MemoryEngine::open(merged_repo.path(), "ci").expect("fresh open");
    assert!(fresh_engine.get_by_topic("billing/tax-rounding").expect("a").is_some());
    assert!(fresh_engine.get_by_topic("android/gradle-daemon").expect("b").is_some());

    let resync = fresh_engine.sync().expect("idempotent resync");
    assert_eq!(resync.files_scanned, 2);
    assert_eq!(resync.files_ingested, 0);
    assert_eq!(resync.records_applied, 0);
}

#[test]
fn staleness_provider_and_changed_files_mark_linked_observations() {
    let tmp = TempDir::new().expect("tempdir");
    let mut engine = MemoryEngine::open(tmp.path(), "alice").expect("open");

    let mut obs_file = sample_input(
        MemoryKind::Decision,
        "ui/button-props",
        "Button Props Contract",
        "Button expects variant=primary|ghost",
    );
    obs_file.related_files = vec!["src/components/Button.tsx".to_owned()];

    let mut obs_sym = sample_input(
        MemoryKind::Domain,
        "core/pricing-fn",
        "Pricing Engine",
        "calculateTotal applies tiered discounts",
    );
    obs_sym.related_symbols = vec!["calculateTotal".to_owned()];

    let saved_file = engine.save(obs_file).expect("save file obs");
    let saved_sym = engine.save(obs_sym).expect("save sym obs");

    let provider = MockFreshnessProvider {
        changed_files: vec!["src/components/Button.tsx".to_owned()],
        missing_symbols: vec!["calculateTotal".to_owned()],
    };
    let updates = engine.check_staleness(&provider).expect("check staleness");
    assert_eq!(updates.len(), 2);

    let reloaded_file = engine.get(&saved_file.id).expect("get").expect("exists");
    let reloaded_sym = engine.get(&saved_sym.id).expect("get").expect("exists");
    assert_eq!(reloaded_file.freshness, FreshnessState::NeedsVerification);
    assert_eq!(reloaded_sym.freshness, FreshnessState::Stale);

    let mut obs_fast = sample_input(
        MemoryKind::Gotcha,
        "api/client-retry",
        "Client Retry",
        "Keep idempotency key header",
    );
    obs_fast.related_files = vec!["./src/api/client.ts".to_owned()];
    let saved_fast = engine.save(obs_fast).expect("save fast");

    let changed = engine
        .mark_files_changed(&["src\\api\\client.ts".to_owned()])
        .expect("mark files changed");
    assert_eq!(changed, 1);
    let reloaded_fast = engine.get(&saved_fast.id).expect("get").expect("exists");
    assert_eq!(reloaded_fast.freshness, FreshnessState::NeedsVerification);
}

#[test]
fn session_ttl_cleanup_removes_only_expired_sessions() {
    let tmp = TempDir::new().expect("tempdir");
    let mut engine = MemoryEngine::open(tmp.path(), "alice").expect("open");

    let mut expired_session = sample_input(
        MemoryKind::Session,
        "session/wip-1",
        "WIP Debugging Auth",
        "Investigating cookie domain mismatch",
    );
    expired_session.ttl_seconds = Some(10);

    let mut active_session = sample_input(
        MemoryKind::Session,
        "session/wip-2",
        "Active Session Note",
        "Testing cache invalidation",
    );
    active_session.ttl_seconds = Some(3600);

    let mut permanent_decision = sample_input(
        MemoryKind::Decision,
        "arch/no-orm",
        "Direct SQL via rusqlite",
        "Avoid heavy ORMs for sub-millisecond queries",
    );
    permanent_decision.ttl_seconds = Some(1);

    let s1 = engine.save(expired_session).expect("save expired");
    let s2 = engine.save(active_session).expect("save active");
    let d1 = engine.save(permanent_decision).expect("save decision");

    let future_ms = s1.updated_at + 60_000;
    let removed = engine
        .cleanup_expired_sessions_at(future_ms)
        .expect("cleanup ttl");
    assert_eq!(removed, 1);

    assert!(engine.get(&s1.id).expect("get s1").is_none());
    assert!(engine.get(&s2.id).expect("get s2").is_some());
    assert!(engine.get(&d1.id).expect("get d1").is_some());
}
