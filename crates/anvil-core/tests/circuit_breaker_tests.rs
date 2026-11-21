use std::sync::Arc;
use std::thread;

use anyhow::Result;
use tempfile::TempDir;

use anvil_core::circuit_breaker::{CircuitBreaker, CircuitState, fingerprint, normalize_error};

#[test]
fn circuit_breaker_trips_exactly_on_third_consecutive_identical_failure() -> Result<()> {
    let tmp = TempDir::new()?;
    let cb = CircuitBreaker::new(tmp.path(), 3);

    let s1 = cb.record_failure("tsc", "src/app.ts(10,5): error TS2322: Type mismatch", "tsc --noEmit")?;
    assert_eq!((s1.state, s1.consecutive_failures), (CircuitState::Closed, 1));

    let s2 = cb.record_failure("tsc", "src\\app.ts(42,12): error TS2322: Type mismatch", "tsc --noEmit")?;
    assert_eq!((s2.state, s2.consecutive_failures), (CircuitState::Closed, 2));

    let s3 = cb.record_failure("tsc", "src/app.ts:99:1 error TS2322: Type mismatch", "tsc --noEmit")?;
    assert_eq!((s3.state, s3.consecutive_failures), (CircuitState::Open, 3));
    let esc = s3.escalation.as_ref().expect("escalation summary present");
    assert_eq!((esc.tool.as_str(), esc.consecutive_failures), ("tsc", 3));
    Ok(())
}

#[test]
fn open_circuit_stays_blocked_until_explicit_reset() -> Result<()> {
    let tmp = TempDir::new()?;
    let cb = CircuitBreaker::new(tmp.path(), 3);
    for _ in 0..3 {
        cb.record_failure("tsc", "src/app.ts(1,1): error TS2322: Type mismatch", "tsc")?;
    }

    assert!(cb.record_success()?.is_open());

    let after_reset = cb.reset()?;
    assert_eq!(after_reset.state, CircuitState::Closed);
    assert_eq!(after_reset.consecutive_failures, 0);
    assert!(after_reset.escalation.is_none());
    Ok(())
}

#[test]
fn different_error_resets_streak_and_success_clears_closed_streak() -> Result<()> {
    let tmp = TempDir::new()?;
    let cb = CircuitBreaker::new(tmp.path(), 3);

    cb.record_failure("eslint", "a.ts:1:1 error no-console", "eslint")?;
    cb.record_failure("eslint", "a.ts:2:1 error no-console", "eslint")?;
    let switched = cb.record_failure("eslint", "b.ts:5:2 error no-unused-vars", "eslint")?;
    assert_eq!(switched.consecutive_failures, 1);
    assert!(!switched.is_open());

    let cleared = cb.record_success()?;
    assert_eq!(cleared.consecutive_failures, 0);
    assert!(cleared.last_fingerprint.is_none());
    assert_eq!(
        normalize_error("C:\\repo\\a.ts(10,3)  0xDEADBEEF"),
        "C:/repo/a.ts:<loc> 0x<addr>"
    );
    Ok(())
}

#[test]
fn timestamp_duration_and_temp_id_normalization_trips_circuit_reliably() -> Result<()> {
    let tmp = TempDir::new()?;
    let cb = CircuitBreaker::new(tmp.path(), 3);

    let e1 = "2026-11-21T10:01:02.111Z FAIL /tmp/run-101 req=550e8400-e29b-41d4-a716-446655440000 in 123ms at src/a.ts:10:2";
    let e2 = "2026-11-21T10:02:09.999Z FAIL /tmp/run-202 req=123e4567-e89b-12d3-a456-426614174000 in 1.45s at src/a.ts:22:9";
    let e3 = "2026-11-21T10:03:45Z FAIL /tmp/run-303 req=999e4567-e89b-12d3-a456-426614174999 in 9ms at src/a.ts(33,1)";

    assert_eq!(fingerprint(e1), fingerprint(e2));
    assert_eq!(fingerprint(e2), fingerprint(e3));

    cb.record_failure("vitest", e1, "npx vitest")?;
    cb.record_failure("vitest", e2, "npx vitest")?;
    let s3 = cb.record_failure("vitest", e3, "npx vitest")?;
    assert_eq!(s3.state, CircuitState::Open);
    assert_eq!(s3.consecutive_failures, 3);
    Ok(())
}

#[test]
fn circuit_breaker_handles_concurrent_updates_via_file_lock() -> Result<()> {
    let tmp = TempDir::new()?;
    let root = Arc::new(tmp.path().to_path_buf());
    let mut handles = Vec::new();
    for _ in 0..4 {
        let r = Arc::clone(&root);
        handles.push(thread::spawn(move || -> Result<()> {
            let cb = CircuitBreaker::new(&r, 10);
            cb.record_failure("tsc", "same error TS2322", "tsc")?;
            Ok(())
        }));
    }
    for h in handles {
        h.join().expect("thread join")?;
    }
    let final_status = CircuitBreaker::new(&root, 10).status()?;
    assert_eq!(final_status.consecutive_failures, 4);
    Ok(())
}
