//! `anvil daemon`: start, stop, and inspect the resident local `anvil-graph` daemon.

use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::cli::subcmds::DaemonSubcmd;
use crate::daemon::protocol::{DaemonRequest, DaemonResponse};
use crate::daemon::{read_state, remove_state, server, state_path, try_request};

const STARTUP_TIMEOUT_MS: u64 = 5_000;
const STOP_TIMEOUT_MS: u64 = 500;

/// Executes `anvil daemon <start|stop|status>`.
pub fn execute(root: &Path, sub: DaemonSubcmd) -> Result<()> {
    match sub {
        DaemonSubcmd::Start {
            background: _,
            foreground,
        } => {
            if foreground {
                server::run_foreground(root)
            } else {
                start_background(root)
            }
        }
        DaemonSubcmd::Stop => stop_daemon(root),
        DaemonSubcmd::Status => status_daemon(root),
    }
}

/// Best-effort lazy background daemon spawn for `SessionStart` hooks (< 5ms, non-blocking).
pub fn ensure_daemon_running_async(root: &Path) {
    if std::env::var_os("ANVIL_NO_AUTO_DAEMON").is_some()
        || !anvil_graph::store::default_bin_path(root).is_file()
        || !root.join(".harness").is_dir()
    {
        return;
    }
    if matches!(try_request(root, DaemonRequest::Ping), Some(DaemonResponse::Pong { .. })) {
        return;
    }
    remove_state(root);
    if let Ok(exe) = std::env::current_exe() {
        let _ = spawn_detached_async(&exe, root);
    }
}

#[cfg(windows)]
fn spawn_detached_async(exe: &Path, root: &Path) -> Result<()> {
    let ps_cmd = "Start-Process -FilePath $env:ANVIL_DAEMON_EXE -ArgumentList @('--root', ('\"{0}\"' -f $env:ANVIL_DAEMON_ROOT), 'daemon', 'start', '--foreground') -WindowStyle Hidden";
    let _child = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", ps_cmd])
        .env("ANVIL_DAEMON_EXE", exe)
        .env("ANVIL_DAEMON_ROOT", root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

#[cfg(not(windows))]
fn spawn_detached_async(exe: &Path, root: &Path) -> Result<()> {
    let _child = Command::new(exe)
        .arg("--root")
        .arg(root)
        .args(["daemon", "start", "--foreground"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    Ok(())
}

fn start_background(root: &Path) -> Result<()> {
    if let Some(DaemonResponse::Pong { pid, .. }) = try_request(root, DaemonRequest::Ping) {
        let port = read_state(root).map_or(0, |s| s.port);
        println!("daemon already running: pid={pid} port={port}");
        return Ok(());
    }

    remove_state(root);
    let exe = std::env::current_exe().context("failed to resolve current anvil executable")?;
    let mut child = spawn_detached_foreground(&exe, root)?;

    let deadline = Instant::now() + Duration::from_millis(STARTUP_TIMEOUT_MS);
    while Instant::now() < deadline {
        if let Some(DaemonResponse::Pong { pid, .. }) = try_request(root, DaemonRequest::Ping) {
            let port = read_state(root).map_or(0, |s| s.port);
            println!("daemon started: pid={pid} port={port}");
            return Ok(());
        }
        if let Some(ref mut proc_child) = child
            && let Ok(Some(status)) = proc_child.try_wait()
        {
            bail!("background daemon exited prematurely with status {status}");
        }
        thread::sleep(Duration::from_millis(25));
    }

    if let Some(mut proc_child) = child {
        let _ = proc_child.kill();
    }
    bail!("background daemon did not become ready within {STARTUP_TIMEOUT_MS}ms")
}

#[cfg(windows)]
fn spawn_detached_foreground(exe: &Path, root: &Path) -> Result<Option<std::process::Child>> {
    // Use Start-Process so the detached daemon does not inherit caller stdout/stderr pipe handles.
    let ps_cmd = "Start-Process -FilePath $env:ANVIL_DAEMON_EXE -ArgumentList @('--root', ('\"{0}\"' -f $env:ANVIL_DAEMON_ROOT), 'daemon', 'start', '--foreground') -WindowStyle Hidden";
    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", ps_cmd])
        .env("ANVIL_DAEMON_EXE", exe)
        .env("ANVIL_DAEMON_ROOT", root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to spawn detached background daemon")?;
    if !status.success() {
        bail!("detached daemon launcher exited with status {status}");
    }
    Ok(None)
}

#[cfg(not(windows))]
fn spawn_detached_foreground(exe: &Path, root: &Path) -> Result<Option<std::process::Child>> {
    let child = Command::new(exe)
        .arg("--root")
        .arg(root)
        .args(["daemon", "start", "--foreground"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to spawn background daemon")?;
    Ok(Some(child))
}

fn stop_daemon(root: &Path) -> Result<()> {
    if read_state(root).is_none() {
        println!("daemon: not running");
        return Ok(());
    }
    match try_request(root, DaemonRequest::Shutdown) {
        Some(DaemonResponse::ShuttingDown) => {
            let path = state_path(root);
            let deadline = Instant::now() + Duration::from_millis(STOP_TIMEOUT_MS);
            while path.exists() && Instant::now() < deadline {
                thread::sleep(Duration::from_millis(20));
            }
            if path.exists() {
                remove_state(root);
            }
            println!("daemon stopped");
        }
        Some(DaemonResponse::Error { message }) => {
            bail!("daemon stop failed: {message}");
        }
        _ => {
            remove_state(root);
            println!("daemon stopped (cleaned stale state)");
        }
    }
    Ok(())
}

fn status_daemon(root: &Path) -> Result<()> {
    let Some(state) = read_state(root) else {
        println!("daemon: stopped");
        return Ok(());
    };
    match try_request(root, DaemonRequest::Ping) {
        Some(DaemonResponse::Pong { pid, nodes, edges }) => {
            println!(
                "daemon: running (pid={pid}, port={}, nodes={nodes}, edges={edges})",
                state.port
            );
        }
        _ => {
            remove_state(root);
            println!("daemon: stopped (cleaned stale state)");
        }
    }
    Ok(())
}
