use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::Result;

use anvil_core::compressor::compress_output;
use anvil_core::proc_util::run_command_with_timeout;

#[test]
fn run_command_with_timeout_passes_fast_command() -> Result<()> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd");
        c.args(["/C", "echo hello-anvil"]);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.args(["-c", "echo hello-anvil"]);
        c
    };

    let out = run_command_with_timeout(&mut cmd, Duration::from_secs(5))?;
    assert!(!out.timed_out);
    assert_eq!(out.status_code, 0);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hello-anvil"));
    Ok(())
}

#[test]
fn run_command_with_timeout_kills_slow_command() -> Result<()> {
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("ping");
        c.args(["-n", "10", "127.0.0.1"]);
        c
    };
    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("sleep");
        c.args(["10"]);
        c
    };

    let out = run_command_with_timeout(&mut cmd, Duration::from_millis(150))?;
    assert!(out.timed_out);
    assert_eq!(out.status_code, 124);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("[anvil] process timed out after 150ms"));
    Ok(())
}

#[test]
fn timeout_kills_grandchildren_and_keeps_partial_output() -> Result<()> {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", "echo partial-line&& ping -n 20 127.0.0.1"]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", "echo partial-line; sleep 20; echo unreachable"]);
        c
    };

    let started = Instant::now();
    let out = run_command_with_timeout(&mut cmd, Duration::from_millis(600))?;
    assert!(out.timed_out);
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "a grandchild holding the pipes must not delay the timeout: {:?}",
        started.elapsed()
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("partial-line") && !stdout.contains("unreachable"));
    Ok(())
}

#[test]
fn compressed_output_names_timeouts_explicitly() -> Result<()> {
    let raw = "test a ... ok

[anvil] process timed out after 300000ms
";
    let out = compress_output(None, "cargo test", 124, raw, 50, 1200)?;
    assert!(!out.passed);
    assert!(out.summary.starts_with("TIMEOUT: killed after 300000ms"), "{}", out.summary);
    assert!(out.lines[0].contains("tier1_timeout_secs"));

    let ordinary = compress_output(None, "cargo test", 1, raw, 50, 1200)?;
    assert!(!ordinary.summary.contains("TIMEOUT"));
    Ok(())
}
