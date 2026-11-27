//! Sandboxed Code Mode dispatcher (`code_exec`) over read-only Anvil MCP tools.

use anvil_codemode::{CodeModePolicy, ToolDispatcher, execute_script};
use anvil_core::stats::{StatsEvent, record_event};
use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::McpServer;
use crate::tools_graph::call_graph_tool;
use crate::tools_memory::call_memory_tool;
use crate::tools_search::call_search_tool;
use crate::tools_tasks::call_task_tool;

pub const CODE_MODE_ALLOWLIST: &[&str] = &[
    "graph_query",
    "graph_explain",
    "graph_outline",
    "graph_read_symbol",
    "graph_impact",
    "graph_origin",
    "graph_path",
    "context_pack",
    "grep",
    "mem_search",
    "mem_timeline",
    "mem_get",
    "mem_context",
    "task_list",
    "task_get",
    "task_resume",
];

struct McpDispatcher<'a>(&'a mut McpServer);

impl ToolDispatcher for McpDispatcher<'_> {
    fn call(&mut self, name: &str, args: Value) -> Result<Value> {
        if !CODE_MODE_ALLOWLIST.contains(&name) {
            bail!("tool `{name}` is not permitted inside `code_exec`");
        }
        if name == "context_pack" || name.starts_with("graph_") {
            call_graph_tool(self.0, name, &args)
        } else if name == "grep" {
            call_search_tool(self.0, name, &args)
        } else if name.starts_with("mem_") {
            call_memory_tool(self.0, name, &args)
        } else if name.starts_with("task_") {
            call_task_tool(self.0, name, &args)
        } else {
            bail!("unsupported code_exec tool: {name}")
        }
    }
}

/// Runs `code_exec` script inside the sandboxed Boa JS engine against read-only Anvil tools.
pub fn call_code_exec(server: &mut McpServer, args: &Value) -> Result<Value> {
    let script = args
        .get("script")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("`script` string is required"))?;
    let max_calls =
        args.get("max_calls").and_then(Value::as_u64).map_or(32, |n| (n as usize).clamp(1, 64));

    let root = server.repo_root().to_path_buf();
    let policy = CodeModePolicy { max_calls, ..CodeModePolicy::default() };
    let mut dispatcher = McpDispatcher(server);
    let res = execute_script(script, CODE_MODE_ALLOWLIST, &mut dispatcher, &policy)?;

    let _ = record_event(
        &root,
        StatsEvent::CodeModeExec {
            calls_executed: res.calls_executed as u64,
            tokens_saved: res.estimated_tokens_saved,
        },
    );

    Ok(json!({
        "result": res.value,
        "calls_executed": res.calls_executed,
        "intermediate_bytes": res.intermediate_bytes,
        "output_bytes": res.output_bytes,
        "estimated_tokens_saved": res.estimated_tokens_saved,
    }))
}
