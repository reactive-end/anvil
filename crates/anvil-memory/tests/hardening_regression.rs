//! Regression tests for Wave H2 hardening in `anvil-memory`.

use std::fs;

use anvil_core::traits::FreshnessState;
use anvil_memory::store::MemoryStore;
use anvil_memory::{MemoryChunkRecord, MemoryEngine, MemoryKind, ObservationInput, SyncStats};
use tempfile::TempDir;

fn make_input(kind: MemoryKind, topic: &str, title: &str, what: &str) -> ObservationInput {
    let mut input = ObservationInput::new(
        kind,
        title,
        what,
        "Hardening concurrency, sync speed, and ephemeral isolation",
        "crates/anvil-memory/src/lib.rs",
        "Keep SQLite WAL busy_timeout and FTS5 triggers minimal",
    );
    input.topic_key = Some(topic.to_owned());
    input
}

#[test]
fn pragma_busy_timeout_and_scoped_trigger_upgrade_cleanly() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join(".harness/.state/anvil.db");
    fs::create_dir_all(db_path.parent().expect("parent")).expect("mkdir");

    // Simulate legacy DB with an unscoped `observations_au` trigger before opening MemoryStore.
    {
        let conn = rusqlite::Connection::open(&db_path).expect("raw sqlite open");
        conn.execute_batch(
            "CREATE TABLE observations (
                id TEXT PRIMARY KEY, topic_key TEXT NOT NULL UNIQUE, kind TEXT NOT NULL,
                title TEXT NOT NULL, what TEXT NOT NULL, why TEXT NOT NULL,
                location TEXT NOT NULL, learned TEXT NOT NULL, related_files TEXT NOT NULL,
                related_symbols TEXT NOT NULL, freshness TEXT NOT NULL, freshness_reason TEXT,
                author TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
                expires_at INTEGER
            );
            CREATE TRIGGER observations_au AFTER UPDATE ON observations BEGIN SELECT 1; END;",
        )
        .expect("legacy schema");
    }

    let store = MemoryStore::open(&db_path).expect("open store");
    let timeout: i64 = store
        .conn()
        .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
        .expect("query busy_timeout");
    assert!(timeout >= 5000, "expected busy_timeout >= 5000, got {timeout}");

    let trigger_sql: String = store
        .conn()
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'trigger' AND name = 'observations_au'",
            [],
            |r| r.get(0),
        )
        .expect("query trigger sql");
    assert!(
        trigger_sql.contains("AFTER UPDATE OF topic_key, title, what, why, location, learned"),
        "expected column-scoped trigger, got: {trigger_sql}"
    );
}

#[test]
fn sync_chunks_skips_unchanged_hash_and_reingests_modified_chunks() {
    let tmp = TempDir::new().expect("tempdir");
    let mut engine = MemoryEngine::open(tmp.path(), "alice").expect("open");
    let saved = engine
        .save(make_input(
            MemoryKind::Decision,
            "sync/hash-check",
            "Hash Checked Sync v1",
            "Skip unchanged BLAKE3 hash and re-ingest when modified",
        ))
        .expect("save");

    let resync = engine.sync().expect("unchanged sync is no-op");
    assert_eq!(resync.files_scanned, 1);
    assert_eq!(resync.files_ingested, 0);
    assert_eq!(resync.records_applied, 0);

    let chunks_dir = tmp.path().join(".harness/memory/chunks");
    let chunk_path = fs::read_dir(&chunks_dir)
        .expect("read chunks")
        .next()
        .expect("one chunk")
        .expect("entry")
        .path();

    // Appending invalid bytes changes the BLAKE3 hash so sync detects tampering instead of skipping.
    fs::write(&chunk_path, b"\xff\xfe not-json").expect("corrupt ingested file");
    assert!(engine.sync().is_err(), "modified chunk with invalid bytes must fail on hash mismatch");
    assert!(engine.get(&saved.id).expect("get").is_some());
}

#[test]
fn sessions_stay_local_and_expired_sessions_are_filtered_from_queries() {
    let tmp = TempDir::new().expect("tempdir");
    let mut engine = MemoryEngine::open(tmp.path(), "alice").expect("open");

    let mut expired = make_input(
        MemoryKind::Session,
        "session/expired-local",
        "Expired Session Secret Keyword",
        "Must stay hidden from search and timeline before cleanup",
    );
    expired.ttl_seconds = Some(-10);

    let mut active = make_input(
        MemoryKind::Session,
        "session/active-local",
        "Active Session Keyword",
        "Active session remains queryable",
    );
    active.ttl_seconds = Some(3600);

    let exp_obs = engine.save(expired).expect("save expired");
    let act_obs = engine.save(active).expect("save active");

    let chunks_dir = tmp.path().join(".harness/memory/chunks");
    let chunk_count = if chunks_dir.exists() {
        fs::read_dir(&chunks_dir).expect("read chunks").count()
    } else {
        0
    };
    assert_eq!(chunk_count, 0, "Session memories must not write Git chunks");

    let fts_hits = engine.search("Keyword", Some(MemoryKind::Session), 10).expect("fts search");
    assert_eq!(fts_hits.len(), 1);
    assert_eq!(fts_hits[0].id, act_obs.id);

    let recent_hits = engine.search("", Some(MemoryKind::Session), 10).expect("recent");
    assert_eq!(recent_hits.len(), 1);
    assert_eq!(recent_hits[0].id, act_obs.id);

    assert!(engine.timeline(&exp_obs.id, 2, 2).expect("tl exp").is_empty());
    let tl_act = engine.timeline(&act_obs.id, 5, 5).expect("tl act");
    assert_eq!(tl_act.len(), 1);
    assert!(engine.get(&exp_obs.id).expect("get exp").is_none());
    assert!(engine.get_by_topic(&exp_obs.topic_key).expect("topic exp").is_none());
}

#[test]
fn expired_legacy_chunk_records_never_resurrect_on_rebuild() {
    let legacy_dir = TempDir::new().expect("legacy tmp");
    let legacy_chunks = legacy_dir.path().join(".harness/memory/chunks");
    fs::create_dir_all(&legacy_chunks).expect("mkdir");
    let legacy_expired = MemoryChunkRecord {
        id: "01JLEGACYEXPIRED0000000001".to_owned(),
        topic_key: "session/legacy-expired".to_owned(),
        kind: MemoryKind::Session,
        title: "Legacy Expired Ghost".to_owned(),
        what: "Should never resurrect".to_owned(),
        why: "Expired".to_owned(),
        location: "src/lib.rs".to_owned(),
        learned: "Skip expired records on sync".to_owned(),
        related_files: vec![],
        related_symbols: vec![],
        author: "alice".to_owned(),
        created_at: 1_000,
        updated_at: 1_000,
        expires_at: Some(2_000),
    };
    let line = format!("{}\n", serde_json::to_string(&legacy_expired).expect("json"));
    fs::write(legacy_chunks.join("alice-01JLEGACYEXPIRED0000000001.jsonl"), line)
        .expect("write legacy chunk");

    let rebuilt = MemoryEngine::open(legacy_dir.path(), "bob").expect("rebuild");
    assert_eq!(rebuilt.last_sync_stats().files_ingested, 1);
    assert_eq!(rebuilt.last_sync_stats().records_applied, 0);
    assert!(rebuilt.get_by_topic("session/legacy-expired").expect("get").is_none());
}

#[test]
fn open_without_sync_defers_sync_and_reports_exact_stats() {
    let src = TempDir::new().expect("src");
    let mut writer = MemoryEngine::open(src.path(), "alice").expect("open writer");
    writer
        .save(make_input(MemoryKind::Decision, "sync/deferred-1", "Deferred One", "First chunk"))
        .expect("save 1");
    writer
        .save(make_input(MemoryKind::Gotcha, "sync/deferred-2", "Deferred Two", "Second chunk"))
        .expect("save 2");

    let dst = TempDir::new().expect("dst");
    let dst_chunks = dst.path().join(".harness/memory/chunks");
    fs::create_dir_all(&dst_chunks).expect("mkdir");
    for entry in fs::read_dir(src.path().join(".harness/memory/chunks")).expect("read") {
        let path = entry.expect("entry").path();
        fs::copy(&path, dst_chunks.join(path.file_name().expect("name"))).expect("copy");
    }

    let mut reader = MemoryEngine::open_without_sync(dst.path(), "bob").expect("open no sync");
    assert_eq!(reader.last_sync_stats(), SyncStats::default());
    assert!(reader.get_by_topic("sync/deferred-1").expect("before sync").is_none());

    let stats = reader.sync().expect("sync");
    assert_eq!(stats, SyncStats { files_scanned: 2, files_ingested: 2, records_applied: 2 });
    assert_eq!(reader.last_sync_stats(), stats);
    assert!(reader.get_by_topic("sync/deferred-2").expect("after sync").is_some());
}

#[test]
fn mark_files_changed_updates_freshness_preserving_fts5_integrity() {
    let tmp = TempDir::new().expect("tempdir");
    let mut engine = MemoryEngine::open(tmp.path(), "alice").expect("open");
    let mut input = make_input(
        MemoryKind::Decision,
        "auth/token-ttl",
        "Token Refresh Strategy",
        "Rotate refresh tokens on every use",
    );
    input.related_files = vec!["crates/auth/refresh.rs".to_owned()];
    let saved = engine.save(input).expect("save");

    let updated = engine
        .mark_files_changed(&["crates/auth/refresh.rs".to_owned()])
        .expect("mark files changed");
    assert_eq!(updated, 1);

    let reloaded = engine.get(&saved.id).expect("get").expect("exists");
    assert_eq!(reloaded.freshness, FreshnessState::NeedsVerification);

    let hits = engine.search("Rotate refresh tokens", None, 5).expect("search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].id, saved.id);
    assert_eq!(hits[0].freshness, FreshnessState::NeedsVerification);

    let db_path = tmp.path().join(".harness/.state/anvil.db");
    let store = MemoryStore::open(&db_path).expect("reopen store");
    store
        .conn()
        .execute("INSERT INTO observations_fts(observations_fts) VALUES('integrity-check')", [])
        .expect("fts5 integrity check");
}

#[test]
fn reopen_skips_ddl_without_mutating_data_version() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join(".harness/.state/anvil.db");

    {
        let mut engine = MemoryEngine::open(tmp.path(), "alice").expect("init");
        engine
            .save(make_input(
                MemoryKind::Decision,
                "schema/version-2",
                "Schema Version Guard",
                "Skip DDL when user_version >= 2",
            ))
            .expect("seed");
    }

    let observer = MemoryStore::open(&db_path).expect("open observer");
    let user_version: i64 =
        observer.conn().query_row("PRAGMA user_version", [], |r| r.get(0)).expect("user_version");
    assert_eq!(user_version, 3);

    let data_version_before: i64 = observer
        .conn()
        .query_row("PRAGMA data_version", [], |r| r.get(0))
        .expect("data_version before");

    let second_open = MemoryStore::open(&db_path).expect("second open skips DDL");
    drop(second_open);

    let data_version_after: i64 = observer
        .conn()
        .query_row("PRAGMA data_version", [], |r| r.get(0))
        .expect("data_version after");
    assert_eq!(
        data_version_after, data_version_before,
        "reopening an initialized DB must not run DDL or bump data_version"
    );
}

#[test]
fn concurrent_reader_and_writer_with_busy_timeout_avoid_sqlite_busy() {
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::Duration;

    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join(".harness/.state/anvil.db");

    {
        let mut engine = MemoryEngine::open(tmp.path(), "alice").expect("init");
        engine
            .save(make_input(
                MemoryKind::Decision,
                "schema/concurrent-rw",
                "Concurrent Reader Writer",
                "WAL + busy_timeout prevents SQLITE_BUSY",
            ))
            .expect("seed");
    }

    let barrier = Arc::new(Barrier::new(2));
    let writer_path = db_path.clone();
    let writer_barrier = Arc::clone(&barrier);
    let writer = thread::spawn(move || {
        let mut store = MemoryStore::open(&writer_path).expect("writer open");
        let tx = store
            .conn_mut()
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .expect("begin immediate");
        writer_barrier.wait();
        thread::sleep(Duration::from_millis(80));
        tx.execute(
            "UPDATE observations SET freshness_reason = 'concurrent-ok' WHERE topic_key = 'schema/concurrent-rw'",
            [],
        )
        .expect("writer update");
        tx.commit().expect("writer commit");
    });

    barrier.wait();
    for _ in 0..4 {
        let reader = MemoryStore::open(&db_path).expect("concurrent reader open");
        let obs = reader
            .get_by_topic_key("schema/concurrent-rw")
            .expect("concurrent read without SQLITE_BUSY")
            .expect("seeded observation");
        assert_eq!(obs.topic_key, "schema/concurrent-rw");
        thread::sleep(Duration::from_millis(25));
    }

    writer.join().expect("writer joined");
}

#[test]
fn concurrent_engine_saves_avoid_sqlite_busy_with_immediate_tx() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    let tmp = TempDir::new().expect("tempdir");
    drop(MemoryEngine::open(tmp.path(), "alice").expect("init schema"));

    let workers = 6usize;
    let barrier = Arc::new(Barrier::new(workers));
    let root = Arc::new(tmp.path().to_path_buf());
    let handles: Vec<_> = (0..workers)
        .map(|i| {
            let root = Arc::clone(&root);
            let barrier = Arc::clone(&barrier);
            thread::spawn(move || {
                let mut eng =
                    MemoryEngine::open_without_sync(&root, format!("dev-{i}")).expect("open");
                barrier.wait();
                eng.save(make_input(
                    MemoryKind::Decision,
                    &format!("concurrent/topic-{i}"),
                    &format!("Concurrent Decision {i}"),
                    "IMMEDIATE transaction acquires write lock cleanly under busy_timeout",
                ))
                .expect("concurrent save");
            })
        })
        .collect();
    for h in handles {
        h.join().expect("worker joined");
    }

    let verifier = MemoryEngine::open(tmp.path(), "ci").expect("reopen");
    for i in 0..workers {
        assert!(verifier.get_by_topic(&format!("concurrent/topic-{i}")).expect("get").is_some());
    }
}

#[test]
fn sync_cleans_deleted_chunk_orphans_expired_ttl_and_preserves_min_created_at() {
    let tmp = TempDir::new().expect("tempdir");
    let mut engine = MemoryEngine::open(tmp.path(), "alice").expect("open");

    let first = engine
        .save(make_input(MemoryKind::Decision, "orphan/topic", "Orphan Target", "Initial"))
        .expect("save orphan candidate");
    let mut expiring =
        make_input(MemoryKind::Decision, "ttl/non-session", "Short Decision", "Body");
    expiring.ttl_seconds = Some(-10);
    engine.save(expiring).expect("save already expired");
    engine.sync().expect("sync purges expired non-session observation");
    assert!(engine.get_by_topic("ttl/non-session").expect("get expired").is_none());

    let older_chunk = MemoryChunkRecord {
        id: "01J00000000000000000000000".to_owned(),
        topic_key: first.topic_key.clone(),
        kind: MemoryKind::Decision,
        title: "Updated Timestamp".to_owned(),
        what: "Updated with earlier created_at".to_owned(),
        why: first.why.clone(),
        location: first.location.clone(),
        learned: first.learned.clone(),
        related_files: vec![],
        related_symbols: vec![],
        author: "bob".to_owned(),
        created_at: first.created_at - 5_000,
        updated_at: first.updated_at + 1_000,
        expires_at: None,
    };
    let chunks_dir = tmp.path().join(".harness/memory/chunks");
    let extra_path = chunks_dir.join("bob-01J00000000000000000000001.jsonl");
    fs::write(&extra_path, format!("{}\n", serde_json::to_string(&older_chunk).unwrap())).unwrap();
    engine.sync().expect("sync newer update");
    let after_up = engine.get_by_topic("orphan/topic").expect("get").expect("exists");
    assert_eq!(after_up.created_at, first.created_at - 5_000);

    fs::remove_file(&extra_path).expect("remove chunk file");
    engine.sync().expect("sync removes orphan");
    assert!(engine.get_by_topic("orphan/topic").expect("after orphan removal").is_none());
}

#[test]
fn v2_sqlite_database_upgrades_cleanly_to_v3() {
    let tmp = TempDir::new().expect("tempdir");
    let db_path = tmp.path().join(".harness/.state/anvil.db");
    fs::create_dir_all(db_path.parent().unwrap()).unwrap();
    let conn = rusqlite::Connection::open(&db_path).expect("open raw v2");
    conn.execute_batch(
        "CREATE TABLE observations (id TEXT PRIMARY KEY, topic_key TEXT NOT NULL UNIQUE, kind TEXT NOT NULL, title TEXT NOT NULL, what TEXT NOT NULL, why TEXT NOT NULL, location TEXT NOT NULL, learned TEXT NOT NULL, related_files TEXT NOT NULL, related_symbols TEXT NOT NULL, freshness TEXT NOT NULL, freshness_reason TEXT, author TEXT NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL, expires_at INTEGER);
         CREATE TABLE ingested_chunks (filename TEXT PRIMARY KEY, blake3_hash TEXT NOT NULL, ingested_at INTEGER NOT NULL);
         PRAGMA user_version = 2;",
    ).expect("seed v2 schema");
    drop(conn);

    let store = MemoryStore::open(&db_path).expect("upgrade to v3");
    let version: i64 = store.conn().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
    assert_eq!(version, 3);
    assert!(store.ingested_chunk_filenames().is_ok());
}
