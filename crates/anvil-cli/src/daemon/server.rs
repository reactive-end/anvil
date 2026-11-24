//! Resident TCP daemon server holding a hot `CodeGraphEngine` in RAM.

use std::fs;
use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use anvil_core::config::AnvilConfig;
use anvil_core::fs_util::atomic_write;
use anvil_graph::{CodeGraphEngine, store};
use chrono::Utc;
use ulid::Ulid;

use super::protocol::{
    DaemonEnvelope, DaemonRequest, DaemonResponse, DaemonState as PersistedDaemonState,
};
use super::{read_state, state_path};

const MAX_CONCURRENT_CONNS: usize = 16;
const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 900;

struct DaemonState {
    engine: CodeGraphEngine,
    disk_sig: Option<(SystemTime, u64)>,
}

struct ConnGuard(Arc<AtomicUsize>);

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Runs the blocking foreground TCP daemon for `repo_root` until `Shutdown` is received.
pub fn run_foreground(repo_root: &Path) -> Result<()> {
    let config = AnvilConfig::load_or_default(repo_root)?;
    let bin_path = store::default_bin_path(repo_root);
    let engine = CodeGraphEngine::load_or_build(repo_root, config)?;
    if !bin_path.is_file() {
        let _ = engine.save();
    }

    let listener = TcpListener::bind("127.0.0.1:0")
        .context("failed to bind local daemon TCP listener on 127.0.0.1:0")?;
    listener
        .set_nonblocking(true)
        .context("failed to configure non-blocking daemon TCP listener")?;
    let port = listener
        .local_addr()
        .context("failed to read local daemon bound port")?
        .port();
    let token = Ulid::new().to_string();
    let pid = std::process::id();

    let persisted = PersistedDaemonState {
        pid,
        port,
        token: token.clone(),
        started_at: Utc::now(),
    };
    let state_bytes = serde_json::to_vec_pretty(&persisted)?;
    atomic_write(&state_path(repo_root), &state_bytes)?;

    let shared_state = Arc::new(Mutex::new(DaemonState {
        engine,
        disk_sig: disk_file_sig(repo_root),
    }));
    let shutdown = Arc::new(AtomicBool::new(false));
    let last_activity = Arc::new(AtomicU64::new(epoch_secs()));
    let idle_secs = resolve_idle_timeout_secs();
    accept_loop(
        &listener,
        &token,
        &shared_state,
        &shutdown,
        &last_activity,
        idle_secs,
    );

    remove_owned_state(repo_root, pid);
    Ok(())
}

fn accept_loop(
    listener: &TcpListener,
    token: &str,
    shared_state: &Arc<Mutex<DaemonState>>,
    shutdown: &Arc<AtomicBool>,
    last_activity: &Arc<AtomicU64>,
    idle_secs: u64,
) {
    let active_conns = Arc::new(AtomicUsize::new(0));
    while !shutdown.load(Ordering::Acquire) {
        if idle_secs > 0 && epoch_secs().saturating_sub(last_activity.load(Ordering::Acquire)) >= idle_secs {
            break;
        }
        match listener.accept() {
            Ok((stream, _)) => {
                last_activity.store(epoch_secs(), Ordering::Release);
                let _ = stream.set_nonblocking(false);
                spawn_or_handle(stream, token, shared_state, shutdown, &active_conns);
            }
            Err(err) if err.kind() == ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => {}
        }
    }
}

fn epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn resolve_idle_timeout_secs() -> u64 {
    std::env::var("ANVIL_DAEMON_IDLE_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_IDLE_TIMEOUT_SECS)
}

fn spawn_or_handle(
    stream: TcpStream,
    token: &str,
    shared_state: &Arc<Mutex<DaemonState>>,
    shutdown: &Arc<AtomicBool>,
    active_conns: &Arc<AtomicUsize>,
) {
    if active_conns.load(Ordering::Acquire) < MAX_CONCURRENT_CONNS {
        active_conns.fetch_add(1, Ordering::AcqRel);
        let guard = ConnGuard(Arc::clone(active_conns));
        let expected_token = token.to_owned();
        let state = Arc::clone(shared_state);
        let sd = Arc::clone(shutdown);
        std::thread::spawn(move || {
            let _guard = guard;
            handle_connection(stream, &expected_token, &state, &sd);
        });
    } else {
        handle_connection(stream, token, shared_state, shutdown);
    }
}

fn handle_connection(
    mut stream: TcpStream,
    expected_token: &str,
    shared_state: &Arc<Mutex<DaemonState>>,
    shutdown: &Arc<AtomicBool>,
) {
    let _ = stream.set_nodelay(true);
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1500)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(1500)));

    let mut line = String::new();
    let mut reader = match stream.try_clone() {
        Ok(cloned) => BufReader::new(cloned),
        Err(_) => return,
    };
    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
        return;
    }

    let response = match serde_json::from_str::<DaemonEnvelope>(&line) {
        Ok(env) if env.token == expected_token => {
            dispatch_request(env.request, shared_state, shutdown)
        }
        Ok(_) => DaemonResponse::Error {
            message: "unauthorized daemon token".to_owned(),
        },
        Err(err) => DaemonResponse::Error {
            message: format!("invalid request JSON: {err}"),
        },
    };

    if let Ok(mut payload) = serde_json::to_vec(&response) {
        payload.push(b'\n');
        let _ = stream.write_all(&payload);
        let _ = stream.flush();
    }
}

fn dispatch_request(
    req: DaemonRequest,
    shared_state: &Arc<Mutex<DaemonState>>,
    shutdown: &Arc<AtomicBool>,
) -> DaemonResponse {
    if matches!(req, DaemonRequest::Shutdown) {
        shutdown.store(true, Ordering::Release);
        return DaemonResponse::ShuttingDown;
    }

    let Ok(mut state) = shared_state.lock() else {
        return DaemonResponse::Error {
            message: "daemon engine lock poisoned".to_owned(),
        };
    };
    refresh_if_disk_changed(&mut state);

    match req {
        DaemonRequest::Ping => DaemonResponse::Pong {
            pid: std::process::id(),
            nodes: state.engine.graph.nodes.len(),
            edges: state.engine.graph.edges.len(),
        },
        DaemonRequest::Shutdown => DaemonResponse::ShuttingDown,
        DaemonRequest::UpdateFile { rel_path } => match state.engine.update_file(&rel_path) {
            Ok(changed) => {
                if changed && state.engine.save().is_ok() {
                    state.disk_sig = disk_file_sig(&state.engine.repo_root);
                }
                DaemonResponse::Updated { rel_path, changed }
            }
            Err(err) => DaemonResponse::Error {
                message: err.to_string(),
            },
        },
        DaemonRequest::Query { term, limit } => DaemonResponse::QueryHits {
            hits: state.engine.query(&term, limit),
        },
        DaemonRequest::Outline { target, max_tokens } => DaemonResponse::OutlineResult {
            outline: state.engine.outline(target.as_deref(), max_tokens),
        },
        DaemonRequest::ReadSymbol {
            symbol,
            include_imports,
        } => match state.engine.read_symbol(&symbol, include_imports) {
            Ok(slice) => DaemonResponse::SymbolSliceResult { slice },
            Err(err) => DaemonResponse::Error {
                message: err.to_string(),
            },
        },
        DaemonRequest::BlastRadius { files, depth } => DaemonResponse::BlastResult {
            report: state.engine.blast_radius(&files, depth),
        },
    }
}

fn disk_file_sig(repo_root: &Path) -> Option<(SystemTime, u64)> {
    let meta = fs::metadata(store::default_bin_path(repo_root)).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn refresh_if_disk_changed(state: &mut DaemonState) {
    let cur = disk_file_sig(&state.engine.repo_root);
    if cur.is_some()
        && cur != state.disk_sig
        && let Ok(reloaded) =
            CodeGraphEngine::load_cached(&state.engine.repo_root, state.engine.config.clone())
    {
        state.engine = reloaded;
        state.disk_sig = cur;
    }
}

fn remove_owned_state(repo_root: &Path, pid: u32) {
    if let Some(st) = read_state(repo_root)
        && st.pid == pid
    {
        let _ = fs::remove_file(state_path(repo_root));
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    use anvil_core::config::{AnvilConfig, StackKind};
    use anvil_graph::CodeGraphEngine;
    use tempfile::TempDir;

    use super::{DaemonState, disk_file_sig, dispatch_request};
    use crate::daemon::protocol::{DaemonRequest, DaemonResponse};

    #[test]
    fn daemon_reloads_graph_when_rebuilt_externally_on_disk() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("src")).unwrap();
        fs::write(
            root.join("src/alpha.ts"),
            "export function alphaOne(): number { return 1; }\n",
        )
        .unwrap();

        let cfg = AnvilConfig {
            stacks: vec![StackKind::TypeScript],
            ..AnvilConfig::default()
        };
        let engine = CodeGraphEngine::build(root, cfg.clone()).unwrap();
        engine.save().unwrap();

        let shared = Arc::new(Mutex::new(DaemonState {
            engine,
            disk_sig: disk_file_sig(root),
        }));
        let shutdown = Arc::new(AtomicBool::new(false));

        // External rebuild adds a brand-new file and saves graph.bin directly to disk.
        fs::write(
            root.join("src/beta.ts"),
            "export function betaFreshSymbol(): number { return 42; }\n",
        )
        .unwrap();
        let rebuilt = CodeGraphEngine::build(root, cfg).unwrap();
        rebuilt.save().unwrap();

        // Dispatching Query should reload graph.bin from disk rather than serving stale RAM.
        let resp = dispatch_request(
            DaemonRequest::Query {
                term: "betaFreshSymbol".to_owned(),
                limit: 5,
            },
            &shared,
            &shutdown,
        );
        match resp {
            DaemonResponse::QueryHits { hits } => {
                assert!(
                    hits.iter().any(|h| h.label.contains("betaFreshSymbol")),
                    "expected externally rebuilt symbol in query hits: {hits:?}"
                );
            }
            other => panic!("unexpected response: {other:?}"),
        }

        // Verify UpdateFile also refreshes disk_sig so stale RAM never overwrites disk.
        fs::write(
            root.join("src/alpha.ts"),
            "export function alphaOne(): number { return 1; }\nexport function alphaTwo(): number { return 2; }\n",
        )
        .unwrap();
        let upd = dispatch_request(
            DaemonRequest::UpdateFile {
                rel_path: "src/alpha.ts".to_owned(),
            },
            &shared,
            &shutdown,
        );
        assert!(matches!(upd, DaemonResponse::Updated { changed: true, .. }));
        let guard = shared.lock().unwrap();
        assert_eq!(guard.disk_sig, disk_file_sig(root));
    }
}
