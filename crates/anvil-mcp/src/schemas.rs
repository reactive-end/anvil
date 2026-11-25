//! JSON Schema definitions for all 22 unified `graph_*`, `context_pack`, `code_exec`, `mem_*`, and `task_*` MCP tools.

use serde_json::{Value, json};

/// Returns JSON Schema definitions for all 22 unified `graph_*`, `context_pack`, `code_exec`, `mem_*`, and `task_*` tools.
pub fn tool_definitions() -> Vec<Value> {
    let mut tools = graph_core_schemas();
    tools.extend(graph_inspect_schemas());
    tools.push(code_exec_schema());
    tools.extend(memory_tool_schemas());
    tools.extend(task_tool_schemas());
    tools
}

fn code_exec_schema() -> Value {
    tool_schema(
        "code_exec",
        "Execute a sandboxed JS snippet (`anvil.<tool>(args)`) to batch/filter multiple read-only graph, memory, and task queries in one turn without flooding context.",
        json!({
            "type": "object",
            "properties": {
                "script": { "type": "string", "description": "JS function body using `anvil.graph_query(...)`, `anvil.mem_search(...)`, etc. and returning a filtered JSON result." },
                "max_calls": { "type": "integer", "minimum": 1, "maximum": 64, "default": 32 }
            },
            "required": ["script"]
        }),
    )
}

fn graph_core_schemas() -> Vec<Value> {
    vec![
        tool_schema(
            "graph_impact",
            "Compute reverse-BFS blast radius and affected tests from changed files.",
            json!({
                "type": "object",
                "properties": {
                    "files": { "type": "array", "items": { "type": "string" }, "description": "Repo-relative changed files (defaults to git working tree diff)." },
                    "depth": { "type": "integer", "minimum": 1, "default": 2 },
                    "typecheck": { "type": "boolean", "default": false }
                }
            }),
        ),
        tool_schema(
            "graph_origin",
            "Trace upstream definition/source-of-truth for anti-slop edits.",
            json!({
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "Symbol name, file path, or graph node ID." }
                },
                "required": ["target"]
            }),
        ),
        tool_schema(
            "graph_query",
            "BM25/token-scored search across code graph symbols, files, and semantic nodes.",
            json!({
                "type": "object",
                "properties": {
                    "term": { "type": "string" },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 10 }
                },
                "required": ["term"]
            }),
        ),
        tool_schema(
            "graph_path",
            "Find shortest directed dependency path between two symbols or files.",
            json!({
                "type": "object",
                "properties": {
                    "from": { "type": "string" },
                    "to": { "type": "string" }
                },
                "required": ["from", "to"]
            }),
        ),
    ]
}

fn graph_inspect_schemas() -> Vec<Value> {
    vec![
        tool_schema(
            "graph_explain",
            "Inspect incoming/outgoing neighbors, community, and god-node status for a node.",
            json!({
                "type": "object",
                "properties": {
                    "target": { "type": "string" }
                },
                "required": ["target"]
            }),
        ),
        tool_schema(
            "graph_read_symbol",
            "Read only the AST line span of a symbol (plus optional imports) without loading the whole file.",
            json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Symbol name, `file::symbol`, or graph node ID." },
                    "include_imports": { "type": "boolean", "default": true }
                },
                "required": ["symbol"]
            }),
        ),
        tool_schema(
            "graph_outline",
            "Return a compact signature-only outline for a file, directory, or repository within a token budget.",
            json!({
                "type": "object",
                "properties": {
                    "target": { "type": "string", "description": "Optional repo-relative file or directory prefix (omit for entire repo)." },
                    "max_tokens": { "type": "integer", "minimum": 1, "maximum": 16000, "default": 1500 }
                }
            }),
        ),
        tool_schema(
            "context_pack",
            "Build a minimal token-budgeted context packet (target outlines, 1-hop dependencies, linked memories, and related tests) for subagent delegation.",
            json!({
                "type": "object",
                "properties": {
                    "targets": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Repo-relative file paths or symbol names."
                    },
                    "max_tokens": { "type": "integer", "minimum": 200, "maximum": 8000, "default": 1200 }
                },
                "required": ["targets"]
            }),
        ),
    ]
}

fn memory_tool_schemas() -> Vec<Value> {
    vec![
        mem_save_schema(),
        tool_schema(
            "mem_search",
            "BM25 FTS5 progressive-disclosure memory search returning compact hits.",
            json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "kind": { "type": "string", "enum": ["decision", "gotcha", "domain", "session"] },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 10 }
                },
                "required": ["query"]
            }),
        ),
        tool_schema(
            "mem_timeline",
            "Return chronological neighborhood around a memory ID or topic_key.",
            json!({
                "type": "object",
                "properties": {
                    "id_or_topic": { "type": "string" },
                    "before": { "type": "integer", "minimum": 0, "default": 5 },
                    "after": { "type": "integer", "minimum": 0, "default": 5 }
                },
                "required": ["id_or_topic"]
            }),
        ),
        tool_schema(
            "mem_get",
            "Load full observation body by ID (or topic_key fallback).",
            json!({
                "type": "object",
                "properties": { "id": { "type": "string" } },
                "required": ["id"]
            }),
        ),
        tool_schema(
            "mem_context",
            "Load token-budgeted session startup memory context.",
            json!({
                "type": "object",
                "properties": {
                    "max_tokens": { "type": "integer", "minimum": 1, "maximum": 8000, "default": 800 }
                }
            }),
        ),
    ]
}

fn mem_save_schema() -> Value {
    tool_schema(
        "mem_save",
        "Persist or upsert a memory observation (decision, gotcha, domain, or session) with secret redaction.",
        json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "enum": ["decision", "gotcha", "domain", "session"] },
                "title": { "type": "string" },
                "what": { "type": "string" },
                "why": { "type": "string" },
                "where": { "type": "string" },
                "learned": { "type": "string" },
                "topic_key": { "type": "string" },
                "related_files": { "type": "array", "items": { "type": "string" } },
                "related_symbols": { "type": "array", "items": { "type": "string" } },
                "ttl_seconds": { "type": "integer" }
            },
            "required": ["kind", "title", "what", "why", "where", "learned"]
        }),
    )
}

fn task_tool_schemas() -> Vec<Value> {
    vec![
        task_start_schema(),
        task_update_schema(),
        task_record_schema(),
        task_done_schema(),
        tool_schema(
            "task_resume",
            "Load the compact crash-recovery brief (`progress.md`, `done_steps`, `next_steps`, `touched_files`, last command result, dirty files) for an interrupted `in_progress` task or explicit `task_id`.",
            json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string", "description": "Optional task ID (`voz/F001` or `T-001`); defaults to active/latest `in_progress` task." }
                }
            }),
        ),
        tool_schema(
            "task_skip",
            "Explicitly skip task creation for minor tweaks or non-task work so noise never enters tasks.json.",
            json!({
                "type": "object",
                "properties": {
                    "reason": { "type": "string", "description": "Brief reason why these edits do not warrant a standalone task." }
                },
                "required": ["reason"]
            }),
        ),
        tool_schema(
            "task_list",
            "List tasks in `.harness/tasks/` (optionally filtered by group or status) with compact summaries without loading full feature specs.",
            json!({
                "type": "object",
                "properties": {
                    "group": { "type": "string", "description": "Optional group subdirectory/project filter (e.g. `voz`, `avatar`)." },
                    "status": { "type": "string", "enum": ["not_started", "in_progress", "blocked", "done"] },
                    "limit": { "type": "integer", "minimum": 1, "maximum": 100, "default": 15 }
                }
            }),
        ),
        tool_schema(
            "task_get",
            "Read full details for a single task/feature ID (`voz/F001` or `T-001`) including `behavior`, `verify`, `evidence`, status, and entries.",
            json!({
                "type": "object",
                "properties": {
                    "task_id": { "type": "string" }
                },
                "required": ["task_id"]
            }),
        ),
    ]
}

fn task_done_schema() -> Value {
    tool_schema(
        "task_done",
        "Complete an existing task ID (updates `progress.md`, `entries/`, and `tasks.json`) or append final verified edits belonging to that deliverable.",
        json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string" },
                "summary": { "type": "string" },
                "evidence": { "type": "string", "description": "Optional verification command/test result." },
                "files": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["task_id", "summary"]
        }),
    )
}

fn task_start_schema() -> Value {
    tool_schema(
        "task_start",
        "Mark an existing task (`task_id`) as `in_progress` or create & start a new tracked task (`title`) before making edits. Writes `progress.md` and updates `tasks.json`.",
        json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string", "description": "Existing queued ID (`voz/F001`, `T-001`) or custom short ID for a new task." },
                "title": { "type": "string", "description": "Concise title when creating a new task." },
                "group": { "type": "string", "description": "Optional subdirectory group under `.harness/tasks/` when creating a new task." },
                "summary": { "type": "string", "description": "Optional initial scope or plan note." }
            }
        }),
    )
}

fn task_update_schema() -> Value {
    tool_schema(
        "task_update",
        "Log incremental progress, completed steps, remaining next steps, or blockers onto a task (updates `progress.md`, `entries/`, `tasks.json`, and active crash-recovery state).",
        json!({
            "type": "object",
            "properties": {
                "task_id": { "type": "string" },
                "summary": { "type": "string" },
                "status": { "type": "string", "enum": ["not_started", "in_progress", "blocked", "done"], "default": "in_progress" },
                "done_steps": { "type": "array", "items": { "type": "string" } },
                "next_steps": { "type": "array", "items": { "type": "string" } },
                "blocker": { "type": "string" },
                "evidence": { "type": "string" },
                "files": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["task_id", "summary"]
        }),
    )
}

fn task_record_schema() -> Value {
    tool_schema(
        "task_record",
        "Create and record a task in one step (e.g. 'Reorganización visual del módulo X'). Prefer `task_start` -> `task_update` -> `task_done` for multi-step work, or `task_skip` for trivial tweaks.",
        json!({
            "type": "object",
            "properties": {
                "group": { "type": "string", "description": "Optional subdirectory group under `.harness/tasks/` (e.g. `voz`)." },
                "task_id": { "type": "string", "description": "Optional short code like `F039` or `AUTH-01`; auto-generates `T-001`, `T-002`, ... when omitted." },
                "title": { "type": "string", "description": "Concise single-line task title." },
                "summary": { "type": "string", "description": "Concise 1-2 sentence technical summary of changes." },
                "status": { "type": "string", "enum": ["not_started", "done", "in_progress", "blocked"], "default": "done" },
                "files": { "type": "array", "items": { "type": "string" } }
            },
            "required": ["title", "summary"]
        }),
    )
}

fn tool_schema(name: &str, description: &str, input_schema: Value) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": input_schema,
    })
}
