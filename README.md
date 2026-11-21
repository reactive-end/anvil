# anvil

Native Rust AI engineering harness for **Antigravity** and **Claude Code**.  
Zero external runtime dependencies — single stripped native binary (`16.86 MB` on Windows x86_64) embedding `tree-sitter` grammars (including Rust), an incremental code graph (`anvil-graph`), SQLite FTS5 + Git-safe persistent memory (`anvil-memory`), unified stdio MCP server (`anvil mcp`), tiered verification, conflict-free team task ledger, and hybrid Web/Mobile visual QA.

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

Every crate is single-responsibility with an acyclic DAG (`crates/`), `unsafe_code = "forbid"`, and a strict `<= 350` line cap per `.rs` file:

| Crate | Responsibility |
| :--- | :--- |
| `anvil-core` | Config (`.harness/config.json`), shared traits (`SymbolFreshnessProvider`, `OutputParser`), stack detector, diff classifier, `N=3` circuit breaker, ratchet engine, git helpers, log compressors, canonical `fs_util` |
| `anvil-ast` | Single-pass embedded `tree-sitter` parser pool (TS/TSX, JS, HTML, CSS, Python, Kotlin, Dart, Rust) and Tier 0 deterministic rules (`escape-hatches`, `comments`, `strict-types`, `barrels`, `root-hygiene`) |
| `anvil-graph` | Parallel incremental code graph (`blake3` + `rayon` + custom `FxHashMap` + `NodeId(u32)` integer interning + `postcard`), framework resolvers (`tsconfig`, Angular, NestJS, CSS, Python, Kotlin, Dart, Rust), label-propagation communities, Tarjan SCC cycles, reverse-BFS blast radius, multi-repo merge |
| `anvil-memory` | Local SQLite FTS5 (`rusqlite` bundled) + conflict-free `.harness/memory/chunks/*.jsonl`, progressive disclosure (`search`/`timeline`/`get`/`context`), secret redaction, staleness check via `SymbolFreshnessProvider` |
| `anvil-mcp` | Unified JSON-RPC 2.0 `stdio` MCP server exposing `graph_*` and `mem_*` tools with mtime-gated graph/chunk caching |
| `anvil-tasks` | Conflict-free team task ledger (`.harness/tasks/<TASK-ID>/entries/<user>-<ulid>.json`), isolated `plan-<user>.md`, deterministic `tasks.json`, and Git merge driver |
| `anvil-adapters` | Thin adapter sync and hook translation for Antigravity (`.agents/`, `AGENTS.md`) and Claude Code (`.claude/`, `CLAUDE.md`, `.mcp.json`) via locked `.harness/.state/inbox.json` |
| `anvil-deps` | Tier 2 dependency staleness/deprecation audits across npm, PyPI, pub.dev, Maven Central/Google Maven, and Cargo |
| `anvil-qa` | Hybrid Web (DOM audit script, Playwright capture runner, console/overflow/overlap checks) and Mobile (`adb logcat`, `uiautomator` XML, Roborazzi) visual QA |
| `anvil-cli` | Thin CLI wiring (`clap`), one module per subcommand |

> **CI Note**: `.github/workflows/ci.yml` lives on local branch `chore/ci-workflow` pending GitHub `workflow` OAuth scope before pushing to `origin`.

## Quick Start & Core Commands

```bash
anvil init                                # Initialize .harness/, thin adapters, git merge driver, graph & tasks.json
anvil sync                                # Non-destructively sync .harness/skills/ and adapter configs
anvil doctor                              # Audit config, adapters, JSONC/extends tsconfig, graph, memory, identity, circuit, tools
anvil check                               # Diff-classified Tier 0 / Tier 1 / Tier 2 verification (--full, --tier 0|1|2, --no-tests)
anvil impact                              # Reverse-BFS blast radius (--depth 2, --origin <target>, --typecheck)
anvil graph build|update|query|path|explain|report|merge
anvil mem save|search|timeline|get|context|sync|check-stale|cleanup
anvil mcp                                 # Start unified stdio MCP server (graph_* + mem_*)
anvil tasks start|update|done|plan|build|list
anvil ratchet log|list|review|test|promote # Governed linter rule testing & promotion (triple filter)
anvil circuit status|reset                # Inspect or reset the N=3 Circuit Breaker
anvil deps [--all] [manifests...]         # Audit dependency age/deprecation
anvil qa onboard|select-web|select-mobile|eval-web|run|scan-mobile
anvil context pack <targets...>           # Minimal token-budgeted subagent context packet
anvil run -- <cmd>                        # Token-compressed command execution with raw log archiving
```

## Antigravity & Claude Code Integration

Running `anvil init` or `anvil sync` non-destructively configures both agents from `.harness/` as the single source of truth:

- **Antigravity**: `.agents/hooks.json` (`SessionStart`, `PreToolUse`, `PostToolUse`, `PreInvocation`, `Stop`), `.agents/mcp_config.json` (`anvil mcp`), `.agents/skills/`, and `AGENTS.md`.
- **Claude Code**: `.claude/settings.json`, `.mcp.json` (`anvil mcp`), `.claude/skills/`, and `CLAUDE.md`.
- **Unified MCP Tools (`anvil mcp`)**: `graph_impact`, `graph_origin`, `graph_query`, `graph_path`, `graph_explain`, `mem_save`, `mem_search`, `mem_timeline`, `mem_get`, `mem_context`.

## Benchmarks (v0.2 Release Build, Windows x86_64)

Measured with `target/release/anvil.exe` (`17,679,360` bytes / `16.86 MB`) on this Rust workspace (`186` `.rs` files, `1,776` nodes, `7,059` edges) and `cargo test --release -p anvil-graph --test fliiper_benchmark -- --nocapture` (read-only against `fliiper-front`: `1,167` files, `8,641` nodes, `24,494` edges):

| Operation | Measured Latency |
| :--- | :--- |
| `anvil --version` (process startup) | `13.77 ms` median |
| `anvil mem search` (CLI process + SQLite FTS5) | `27.60 ms` median |
| `anvil graph query` (CLI process + `NodeId(u32)` graph load + BM25) | `19.61 ms` median |
| `fliiper-front` cold graph build (`1,167` files / `8,641` nodes / `24,494` edges) | `460.34 ms` |
| `fliiper-front` binary save / load (`postcard` + `NodeId(u32)`) | `11.10 ms` save / `25.68 ms` load |
| `fliiper-front` incremental 1-file update | `4.41 ms` |
| `fliiper-front` reverse-BFS blast radius (`depth=2`, `impacted=5`) | `0.27 ms` |
