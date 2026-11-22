//! Resident TCP daemon server holding a hot `CodeGraphEngine` in RAM.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use anvil_core::config::AnvilConfig;
use anvil_core::fs_util::atomic_write;
use anvil_graph::{CodeGraphEngine, store};
use chrono::Utc;
use ulid::Ulid;

use super::protocol::{DaemonEnvelope, DaemonRequest, DaemonResponse, DaemonState};
use super::{read_state, state_path};

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
    let port = listener
        .local_addr()
        .context("failed to read local daemon bound port")?
        .port();
    let token = Ulid::new().to_string();
    let pid = std::process::id();

    let state = DaemonState {
        pid,
        port,
        token: token.clone(),
        started_at: Utc::now(),
    };
    let state_bytes = serde_json::to_vec_pretty(&state)?;
    atomic_write(&state_path(repo_root), &state_bytes)?;

    let shared_engine = Arc::new(Mutex::new(engine));
    let shutdown = Arc::new(AtomicBool::new(false));

    for incoming in listener.incoming() {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        let Ok(stream) = incoming else {
            continue;
        };
        handle_connection(stream, &token, &shared_engine, &shutdown);
        if shutdown.load(Ordering::Acquire) {
            break;
        }
    }

    remove_owned_state(repo_root, pid);
    Ok(())
}

fn handle_connection(
    mut stream: TcpStream,
    expected_token: &str,
    shared_engine: &Arc<Mutex<CodeGraphEngine>>,
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
            dispatch_request(env.request, shared_engine, shutdown)
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
    shared_engine: &Arc<Mutex<CodeGraphEngine>>,
    shutdown: &Arc<AtomicBool>,
) -> DaemonResponse {
    if matches!(req, DaemonRequest::Shutdown) {
        shutdown.store(true, Ordering::Release);
        return DaemonResponse::ShuttingDown;
    }

    let Ok(mut engine) = shared_engine.lock() else {
        return DaemonResponse::Error {
            message: "daemon engine lock poisoned".to_owned(),
        };
    };

    match req {
        DaemonRequest::Ping => DaemonResponse::Pong {
            pid: std::process::id(),
            nodes: engine.graph.nodes.len(),
            edges: engine.graph.edges.len(),
        },
        DaemonRequest::Shutdown => DaemonResponse::ShuttingDown,
        DaemonRequest::UpdateFile { rel_path } => match engine.update_file(&rel_path) {
            Ok(changed) => {
                if changed {
                    let _ = engine.save();
                }
                DaemonResponse::Updated { rel_path, changed }
            }
            Err(err) => DaemonResponse::Error {
                message: err.to_string(),
            },
        },
        DaemonRequest::Query { term, limit } => DaemonResponse::QueryHits {
            hits: engine.query(&term, limit),
        },
        DaemonRequest::Outline { target, max_tokens } => DaemonResponse::OutlineResult {
            outline: engine.outline(target.as_deref(), max_tokens),
        },
        DaemonRequest::ReadSymbol {
            symbol,
            include_imports,
        } => match engine.read_symbol(&symbol, include_imports) {
            Ok(slice) => DaemonResponse::SymbolSliceResult { slice },
            Err(err) => DaemonResponse::Error {
                message: err.to_string(),
            },
        },
        DaemonRequest::BlastRadius { files, depth } => DaemonResponse::BlastResult {
            report: engine.blast_radius(&files, depth),
        },
    }
}

fn remove_owned_state(repo_root: &Path, pid: u32) {
    if let Some(st) = read_state(repo_root)
        && st.pid == pid
    {
        let _ = fs::remove_file(state_path(repo_root));
    }
}
