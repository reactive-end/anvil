use std::process::Command;
use std::time::Duration;

use anyhow::Result;

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
