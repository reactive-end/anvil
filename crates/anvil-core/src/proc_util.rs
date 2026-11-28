//! Cross-platform subprocess execution with wall-clock timeout and deadlock-free pipe draining.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// Exit code reported for a process killed by its wall-clock timeout.
pub const TIMEOUT_EXIT_CODE: i32 = 124;
/// Marker preceding the limit in the stderr note appended to a killed process.
pub const TIMEOUT_MARKER: &str = "[anvil] process timed out after ";

const KILLED_PIPE_GRACE: Duration = Duration::from_secs(2);

/// Captured output and exit status of a timeout-bounded subprocess run.
#[derive(Debug, Clone)]
pub struct TimedOutput {
    pub status_code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub timed_out: bool,
}

/// Executes `cmd` with null stdin, piped stdout/stderr drained by background threads,
/// and a hard wall-clock `timeout`. On timeout, kills the whole process tree and returns exit
/// code `124` with the output captured so far.
pub fn run_command_with_timeout(cmd: &mut Command, timeout: Duration) -> Result<TimedOutput> {
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd.spawn().context("failed to spawn subprocess")?;
    let stdout_reader = PipeReader::spawn(child.stdout.take());
    let stderr_reader = PipeReader::spawn(child.stderr.take());

    let deadline = Instant::now() + timeout;
    let mut sleep_ms = 5u64;

    loop {
        if let Some(status) = child.try_wait().context("failed to poll subprocess")? {
            let stdout = stdout_reader.finish(None);
            let stderr = stderr_reader.finish(None);
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

    kill_process_tree(&mut child);
    let stdout = stdout_reader.finish(Some(KILLED_PIPE_GRACE));
    let mut stderr = stderr_reader.finish(Some(KILLED_PIPE_GRACE));
    stderr.extend_from_slice(format!("\n{TIMEOUT_MARKER}{}ms\n", timeout.as_millis()).as_bytes());
    Ok(TimedOutput { status_code: TIMEOUT_EXIT_CODE, stdout, stderr, timed_out: true })
}

/// Kills the child and every descendant so builds and test runners do not outlive the timeout.
/// Reaps the child; also used to stop long-lived servers together with whatever they spawned.
pub fn kill_process_tree(child: &mut Child) {
    if cfg!(windows) {
        run_quiet("taskkill", &["/T", "/F", "/PID", &child.id().to_string()]);
    } else {
        let mut args = vec!["-KILL".to_owned()];
        args.extend(descendant_pids(child.id()).iter().map(u32::to_string));
        if args.len() > 1 {
            run_quiet("kill", &args.iter().map(String::as_str).collect::<Vec<_>>());
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn run_quiet(program: &str, args: &[&str]) {
    let _ = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn descendant_pids(root: u32) -> Vec<u32> {
    let Ok(out) = Command::new("ps").args(["-A", "-o", "pid=,ppid="]).output() else {
        return Vec::new();
    };
    let table: Vec<(u32, u32)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let mut cols = line.split_whitespace().map(str::parse::<u32>);
            Some((cols.next()?.ok()?, cols.next()?.ok()?))
        })
        .collect();
    let mut found = Vec::new();
    let mut frontier = vec![root];
    while let Some(parent) = frontier.pop() {
        for (pid, _) in table.iter().filter(|(_, ppid)| *ppid == parent) {
            if !found.contains(pid) {
                found.push(*pid);
                frontier.push(*pid);
            }
        }
    }
    found
}

/// Drains a pipe on a background thread, keeping partial output readable at any time.
struct PipeReader {
    buf: Arc<Mutex<Vec<u8>>>,
    handle: JoinHandle<()>,
}

impl PipeReader {
    fn spawn<R: Read + Send + 'static>(pipe: Option<R>) -> Self {
        let buf = Arc::new(Mutex::new(Vec::new()));
        let shared = Arc::clone(&buf);
        let handle = thread::spawn(move || {
            let Some(mut reader) = pipe else {
                return;
            };
            let mut chunk = [0u8; 8192];
            while let Ok(read) = reader.read(&mut chunk) {
                if read == 0 {
                    break;
                }
                lock_buf(&shared).extend_from_slice(&chunk[..read]);
            }
        });
        Self { buf, handle }
    }

    /// Returns the captured bytes, waiting for EOF or at most `grace` when one is given.
    fn finish(self, grace: Option<Duration>) -> Vec<u8> {
        if let Some(grace) = grace {
            let deadline = Instant::now() + grace;
            while !self.handle.is_finished() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(5));
            }
        } else {
            let _ = self.handle.join();
        }
        std::mem::take(&mut *lock_buf(&self.buf))
    }
}

fn lock_buf(buf: &Mutex<Vec<u8>>) -> MutexGuard<'_, Vec<u8>> {
    buf.lock().unwrap_or_else(PoisonError::into_inner)
}
