# anvil

Native Rust AI engineering harness for **Antigravity** and **Claude Code**.  
Zero external runtime dependencies — single stripped native binary (`15.52 MB` on Windows x86_64) embedding `tree-sitter` grammars, an incremental code graph (`anvil-graph`), SQLite FTS5 + Git-safe persistent memory (`anvil-memory`), unified stdio MCP server (`anvil mcp`), tiered verification, conflict-free team task ledger, and hybrid Web/Mobile visual QA.

## Installation

### Windows (PowerShell)

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\install.ps1
```

Installs `anvil.exe` to `~/.cargo/bin` (or `$env:LOCALAPPDATA\anvil\bin`), ensures `PATH` is configured, and verifies `anvil --version`.

### macOS / Linux

```bash
./scripts/install.sh
```

Installs `anvil` to `~/.cargo/bin` (or `~/.local/bin`) and verifies `anvil --version`.

### Manual Release Build

```bash
cargo build --release -p anvil-cli
```

Release profile uses `opt-level = 3`, `lto = "fat"`, `codegen-units = 1`, `strip = true`, and `panic = "abort"`.

## Architecture (10 SRP Crates)

Every crate is single-responsibility with an acyclic DAG (`crates/`), `unsafe_code = "forbid"`, and a strict `<= 350` line cap per `.rs` file enforced in CI:

| Crate | Responsibility |
| :--- | :--- |
| `anvil-core` | Config (`.harness/config.json`), shared traits (`SymbolFreshnessProvider`, `OutputParser`), stack detector, diff classifier, `N=3` circuit breaker, ratchet engine, git helpers, log compressors |
| `anvil-ast` | Single-pass embedded `tree-sitter` parser pool (TS/TSX, JS, HTML, CSS, Python, Kotlin, Dart) and Tier 0 deterministic rules (`escape-hatches`, `comments`, `strict-types`, `barrels`, `root-hygiene`) |
| `anvil-graph` | Parallel incremental code graph (`blake3` + `rayon` + `petgraph` + `postcard`), framework resolvers (`tsconfig`, Angular, NestJS, Python, Kotlin, Dart), Louvain communities, reverse-BFS blast radius, multi-repo merge |
| `anvil-memory` | Local SQLite FTS5 (`rusqlite` bundled) + conflict-free `.harness/memory/chunks/*.jsonl`, progressive disclosure (`search`/`timeline`/`get`/`context`), secret redaction, staleness check via `SymbolFreshnessProvider` |
| `anvil-mcp` | Unified JSON-RPC 2.0 `stdio` MCP server exposing `graph_*` and `mem_*` tools |
| `anvil-tasks` | Conflict-free team task ledger (`.harness/tasks/<TASK-ID>/entries/<user>-<ulid>.json`), isolated `plan-<user>.md`, deterministic `tasks.json`, and Git merge driver |
| `anvil-adapters` | Thin adapter sync and hook translation for Antigravity (`.agents/`, `AGENTS.md`) and Claude Code (`.claude/`, `CLAUDE.md`, `.mcp.json`) via `.harness/.state/inbox.json` |
| `anvil-deps` | Tier 2 dependency staleness/deprecation audits across npm, PyPI, pub.dev, and Maven Central |
| `anvil-qa` | Hybrid Web (DOM audit script + console/overflow/overlap checks) and Mobile (`adb logcat`, `uiautomator` XML, Roborazzi) visual QA |
| `anvil-cli` | Thin CLI wiring (`clap`), one module per subcommand |

## Quick Start & Core Commands

```bash
anvil init                                # Initialize .harness/, thin adapters, git merge driver, graph & tasks.json
anvil sync                                # Non-destructively sync .harness/skills/ and adapter configs
anvil doctor                              # Audit config, adapters, strict TS, graph, memory, identity, circuit, tools
anvil check                               # Diff-classified Tier 0 / Tier 1 / Tier 2 verification (--full, --tier 0|1|2)
anvil impact                              # Reverse-BFS blast radius (--depth 2, --origin <target>, --typecheck)
anvil graph build|update|query|path|explain|report|merge
anvil mem save|search|timeline|get|context|sync|check-stale|cleanup
anvil mcp                                 # Start unified stdio MCP server (graph_* + mem_*)
anvil tasks start|update|done|plan|build|list
anvil ratchet log|list|review|promote     # Governed linter rule promotion (triple filter)
anvil circuit status|reset                # Inspect or reset the N=3 Circuit Breaker
anvil deps [--all] [manifests...]         # Audit dependency age/deprecation
anvil qa onboard|select-web|select-mobile|eval-web|scan-mobile
anvil context pack <targets...>           # Minimal token-budgeted subagent context packet
anvil run -- <cmd>                        # Token-compressed command execution with raw log archiving
```

## Antigravity & Claude Code Integration

Running `anvil init` or `anvil sync` non-destructively configures both agents from `.harness/` as the single source of truth:

- **Antigravity**: `.agents/hooks.json` (`SessionStart`, `PreToolUse`, `PostToolUse`, `PreInvocation`, `Stop`), `.agents/mcp_config.json` (`anvil mcp`), `.agents/skills/`, and `AGENTS.md`.
- **Claude Code**: `.claude/settings.json`, `.mcp.json` (`anvil mcp`), `.claude/skills/`, and `CLAUDE.md`.
- **Unified MCP Tools (`anvil mcp`)**: `graph_impact`, `graph_origin`, `graph_query`, `graph_path`, `graph_explain`, `mem_save`, `mem_search`, `mem_timeline`, `mem_get`, `mem_context`.

## Benchmarks (Release Build, Windows x86_64)

Measured with `target/release/anvil.exe` (`16,268,800` bytes / `15.52 MB`) and `cargo test --release -p anvil-graph --test fliiper_benchmark -- --nocapture` (read-only against `fliiper-front`: 1,165 files, 8,633 nodes, 44,884 edges):

| Operation | Measured Latency |
| :--- | :--- |
| `anvil --version` (process startup) | `8.01 ms` median (`7.67 ms` min, `15.08 ms` cold) |
| `anvil mem search` (CLI process + SQLite FTS5) | `44.94 ms` median |
| `anvil graph query` (CLI process + graph load + BM25) | `54.96 ms` median |
| `fliiper-front` cold graph build (`1,165` files / `8,633` nodes / `44,884` edges) | `322.86 ms` |
| `fliiper-front` binary save / load (`postcard`) | `13.13 ms` save / `48.92 ms` load |
| `fliiper-front` incremental 1-file update | `3.05 ms` |
| `fliiper-front` reverse-BFS blast radius (`depth=2`, `impacted=5`) | `0.29 ms` |
