//! Unified stdio MCP server exposing `graph_*` and `mem_*` tools.

pub mod router;
pub mod tools_graph;
pub mod tools_memory;
pub mod transport;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::SystemTime;

use anyhow::Result;
use anvil_core::config::AnvilConfig;
use anvil_graph::{CodeGraphEngine, store};
use anvil_memory::MemoryEngine;
use serde::Deserialize;

use crate::transport::{JsonRpcRequest, JsonRpcResponse};

/// Stateful MCP server lazily initializing `anvil-graph` and `anvil-memory`.
pub struct McpServer {
    repo_root: PathBuf,
    config: AnvilConfig,
    graph: Option<CodeGraphEngine>,
    graph_bin_mtime: Option<SystemTime>,
    memory: Option<MemoryEngine>,
    initialized: bool,
}

impl McpServer {
    /// Creates a new MCP server bound to `repo_root` with fast (< 5ms) lazy engine loading.
    pub fn new(repo_root: impl AsRef<Path>) -> Result<Self> {
        let root = repo_root.as_ref().to_path_buf();
        let config = AnvilConfig::load_or_default(&root)?;
        Ok(Self {
            repo_root: root,
            config,
            graph: None,
            graph_bin_mtime: None,
            memory: None,
            initialized: false,
        })
    }

    /// Returns the repository root path.
    pub fn repo_root(&self) -> &Path {
        &self.repo_root
    }

    /// Returns whether the client completed MCP initialization.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    /// Marks the MCP session as initialized.
    pub fn mark_initialized(&mut self) {
        self.initialized = true;
    }

    /// Dispatches a single JSON-RPC 2.0 request/notification.
    pub fn handle_message(&mut self, req: &JsonRpcRequest) -> Option<JsonRpcResponse> {
        router::dispatch(self, req)
    }

    /// Runs the buffered stdio JSON-RPC 2.0 server loop until EOF.
    pub fn run_stdio(&mut self) -> Result<()> {
        transport::run_stdio_loop(|req| self.handle_message(req))
    }

    /// Lazily loads (or reloads if updated externally on disk) the `CodeGraphEngine`.
    pub fn ensure_graph(&mut self) -> Result<&mut CodeGraphEngine> {
        let bin_path = store::default_bin_path(&self.repo_root);
        let disk_mtime = fs::metadata(&bin_path).and_then(|m| m.modified()).ok();
        let needs_reload = self.graph.is_none() || disk_mtime != self.graph_bin_mtime;

        if needs_reload {
            let engine = CodeGraphEngine::load_or_build(&self.repo_root, self.config.clone())?;
            if !bin_path.exists() {
                let _ = engine.save();
            }
            self.graph_bin_mtime = fs::metadata(&bin_path).and_then(|m| m.modified()).ok();
            self.graph = Some(engine);
        }

        self.graph
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("code graph failed to initialize"))
    }

    /// Lazily opens the repository `MemoryEngine`.
    pub fn ensure_memory(&mut self) -> Result<&mut MemoryEngine> {
        if self.memory.is_none() {
            let author = resolve_cached_author(&self.repo_root);
            let engine = MemoryEngine::open(&self.repo_root, author)?;
            self.memory = Some(engine);
        }
        self.memory
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("memory engine failed to initialize"))
    }
}

/// Runs the unified `anvil mcp` stdio server for `repo_root`.
pub fn run_stdio_server(repo_root: &Path) -> Result<()> {
    let mut server = McpServer::new(repo_root)?;
    server.run_stdio()
}

#[derive(Deserialize)]
struct CachedIdentity {
    author: String,
}

fn resolve_cached_author(repo_root: &Path) -> String {
    let cache_path = repo_root.join(".harness/.state/identity.json");
    if let Ok(raw) = fs::read_to_string(cache_path)
        && let Ok(parsed) = serde_json::from_str::<CachedIdentity>(&raw)
        && !parsed.author.trim().is_empty()
    {
        return parsed.author;
    }

    if let Ok(out) = Command::new("git")
        .args(["config", "user.name"])
        .current_dir(repo_root)
        .output()
        && out.status.success()
    {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if !name.is_empty() {
            return name;
        }
    }

    "mcp".to_owned()
}
