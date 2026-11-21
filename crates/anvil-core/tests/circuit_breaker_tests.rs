use anyhow::Result;
use tempfile::TempDir;

use anvil_core::circuit_breaker::{CircuitBreaker, CircuitState, normalize_error};

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
