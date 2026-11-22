//! Resident local daemon client helpers and state management.

pub mod protocol;
pub mod server;

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use protocol::{DaemonEnvelope, DaemonRequest, DaemonResponse, DaemonState};

/// Returns the canonical daemon metadata path `.harness/.state/daemon.json`.
pub fn state_path(repo_root: &Path) -> PathBuf {
    repo_root
        .join(".harness")
        .join(".state")
        .join("daemon.json")
}

/// Reads persisted daemon connection metadata if present and valid.
pub fn read_state(repo_root: &Path) -> Option<DaemonState> {
    let raw = fs::read_to_string(state_path(repo_root)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Removes `.harness/.state/daemon.json` when cleaning up stale or stopped daemon state.
pub fn remove_state(repo_root: &Path) {
    let _ = fs::remove_file(state_path(repo_root));
}

/// Attempts a fast (< 50ms connect timeout) authenticated IPC request against a running daemon.
/// Returns `None` if no daemon is running or the socket cannot be reached so callers can fall back to disk.
pub fn try_request(repo_root: &Path, request: DaemonRequest) -> Option<DaemonResponse> {
    let state = read_state(repo_root)?;
    let addr = SocketAddr::from(([127, 0, 0, 1], state.port));
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(50)).ok()?;
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));

    let envelope = DaemonEnvelope {
        token: state.token,
        request,
    };
    let mut bytes = serde_json::to_vec(&envelope).ok()?;
    bytes.push(b'\n');
    stream.write_all(&bytes).ok()?;
    stream.flush().ok()?;

    let mut reader = BufReader::new(stream);
    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).ok()?;
    if resp_line.trim().is_empty() {
        return None;
    }
    serde_json::from_str(&resp_line).ok()
}
