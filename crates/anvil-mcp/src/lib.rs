//! Unified stdio MCP server exposing `graph_*` and `mem_*` tools.

pub mod router;
pub mod tools_graph;
pub mod tools_memory;
pub mod transport;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use anvil_core::config::AnvilConfig;
use anvil_graph::{CodeGraphEngine, store};
use anvil_memory::MemoryEngine;
use anvil_tasks::resolve_author_fast;

use crate::transport::{JsonRpcRequest, JsonRpcResponse};

/// Stateful MCP server lazily initializing `anvil-graph` and `anvil-memory`.
pub struct McpServer {
    repo_root: PathBuf,
    config: AnvilConfig,
    graph: Option<CodeGraphEngine>,
    graph_bin_mtime: Option<SystemTime>,
    memory: Option<MemoryEngine>,
    chunks_dir_mtime: Option<SystemTime>,
    chunks_synced_once: bool,
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
            chunks_dir_mtime: None,
            chunks_synced_once: false,
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

    /// Lazily loads (or reloads if `graph.bin` mtime changed) the cached `CodeGraphEngine`.
    pub fn ensure_graph(&mut self) -> Result<&mut CodeGraphEngine> {
        let bin_path = store::default_bin_path(&self.repo_root);
        let disk_mtime = fs::metadata(&bin_path).and_then(|m| m.modified()).ok();
        let needs_reload = self.graph.is_none() || disk_mtime != self.graph_bin_mtime;

        if needs_reload {
            let engine = CodeGraphEngine::load_cached(&self.repo_root, self.config.clone())?;
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

    /// Lazily opens `MemoryEngine` and syncs only when `.harness/memory/chunks/` mtime changed.
    pub fn ensure_memory(&mut self) -> Result<&mut MemoryEngine> {
        self.ensure_memory_internal(true)
    }

    /// Lazily opens `MemoryEngine` without scanning chunks (for `mem_save`).
    pub fn ensure_memory_no_sync(&mut self) -> Result<&mut MemoryEngine> {
        self.ensure_memory_internal(false)
    }

    /// Records current `.harness/memory/chunks/` mtime after a local `mem_save`.
    pub fn refresh_chunks_mtime(&mut self) {
        let chunks_dir = self.repo_root.join(".harness/memory/chunks");
        self.chunks_dir_mtime = fs::metadata(&chunks_dir).and_then(|m| m.modified()).ok();
        self.chunks_synced_once = self.chunks_dir_mtime.is_some();
    }

    fn ensure_memory_internal(&mut self, sync_if_changed: bool) -> Result<&mut MemoryEngine> {
        if self.memory.is_none() {
            let author = resolve_author_fast(&self.repo_root).unwrap_or_else(|_| "mcp".to_owned());
            let engine = MemoryEngine::open_without_sync(&self.repo_root, author)?;
            self.memory = Some(engine);
        }
        let mem = self
            .memory
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("memory engine failed to initialize"))?;

        if sync_if_changed {
            let chunks_dir = self.repo_root.join(".harness/memory/chunks");
            let disk_mtime = fs::metadata(&chunks_dir).and_then(|m| m.modified()).ok();
            if disk_mtime.is_some()
                && (!self.chunks_synced_once || disk_mtime != self.chunks_dir_mtime)
            {
                mem.sync()?;
                self.chunks_dir_mtime = disk_mtime;
                self.chunks_synced_once = true;
            }
        }
        Ok(mem)
    }
}

/// Runs the unified `anvil mcp` stdio server for `repo_root`.
pub fn run_stdio_server(repo_root: &Path) -> Result<()> {
    let mut server = McpServer::new(repo_root)?;
    server.run_stdio()
}
