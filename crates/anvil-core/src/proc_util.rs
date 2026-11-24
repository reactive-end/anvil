//! Cross-platform subprocess execution with wall-clock timeout and deadlock-free pipe draining.

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// Captured output and exit status of a timeout-bounded subprocess run.
#[derive(Debug, Clone)]
pub struct TimedOutput {
    pub status_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

/// Executes `cmd` with null stdin, piped stdout/stderr drained by background threads,
/// and a hard wall-clock `timeout`. On timeout, kills the child and returns exit code `124`.
pub fn run_command_with_timeout(cmd: &mut Command, timeout: Duration) -> Result<TimedOutput> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().context("failed to spawn subprocess")?;
    let stdout_handle = spawn_pipe_reader(child.stdout.take());
    let stderr_handle = spawn_pipe_reader(child.stderr.take());

    let deadline = Instant::now() + timeout;
    let mut sleep_ms = 5u64;

    loop {
        if let Some(status) = child.try_wait().context("failed to poll subprocess")? {
            let stdout = stdout_handle.join().unwrap_or_default();
            let stderr = stderr_handle.join().unwrap_or_default();
            return Ok(TimedOutput {
                status_code: status.code().unwrap_or(1),
                stdout,
                stderr,
                timed_out: false,
            });
        }

        let now = Instant::now();
        if now >= deadline {
            break;
        }
        let remaining = deadline.saturating_duration_since(now);
        thread::sleep(Duration::from_millis(sleep_ms).min(remaining));
        sleep_ms = (sleep_ms * 2).min(20);
    }

    let _ = child.kill();
    let _ = child.wait();
    let stdout = stdout_handle.join().unwrap_or_default();
    let mut stderr = stderr_handle.join().unwrap_or_default();
    stderr.extend_from_slice(
        format!("\n[anvil] process timed out after {}ms\n", timeout.as_millis()).as_bytes(),
    );
    Ok(TimedOutput {
        status_code: 124,
        stdout,
        stderr,
        timed_out: true,
    })
}

fn spawn_pipe_reader<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut buf = Vec::new();
        if let Some(mut reader) = pipe {
            let _ = reader.read_to_end(&mut buf);
        }
        buf
    })
}
