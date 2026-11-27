<!-- anvil:managed:start -->

# Anvil Rules — Cursor Thin Adapter

> **Availability Guard**: Only apply this `anvil:managed` block if the `anvil` binary is on `PATH` (or the `anvil` MCP server is active). If `anvil` is not installed in this environment, **skip this entire block**—do NOT invoke `anvil` commands or treat `.harness/` as an active workflow/source of truth; use the repository's native tools instead.
> Single source of truth (when `anvil` is available): `.harness/` (`config.json`, `rules/`, `candidates/`, `skills/`, `memory/chunks/`, `tasks/`). Synced via `anvil sync` (`.cursor/mcp.json`, `.cursor/rules/anvil.mdc`, `.cursor/skills/`).

## Core Discipline

1. **Deterministic Commands (`anvil`)**:
   - `anvil check` — Tiered verification (`Tier 0` AST, `Tier 1` impact + typecheck + targeted tests, `Tier 2` visual QA & deps).
   - `anvil run -- <cmd>` — Token-compressed runner (1-line PASS; <= 50-line failure; raw log in `.harness/.state/logs/`).
   - `anvil impact <files> --origin <target>` — Blast-radius via `anvil-graph`; fix the origin, never patch downstream.
   - `anvil diff` — Token-compressed diff. `anvil grep <pattern> [paths] [-i -F -w -g <glob> -C <n> -l]` — fast gitignore-aware code search grouped by file, each match annotated with its enclosing symbol when a graph exists (`anvil graph build`); also the MCP `grep` tool. Use `anvil daemon start|stop|status` & `anvil stats`.
   - `anvil circuit status|reset` — Circuit breaker trips at `N=3` identical failures; inspect root cause before resetting.
   - `anvil tasks start|update|done|resume|verify|record|skip|build` & `anvil mem save|search` — Crash-safe tasks & persistent memory.
   - `anvil ratchet log|list|review|test|promote`, `anvil deps`, `anvil qa run|onboard|eval-web|scan-mobile`, `anvil doctor`. Living docs (OKF) in `.harness/docs/<project>/`: read `index.md` and the module's concept before touching a module; after behaviour changes update the concept and run `anvil docs refresh <id>` (`anvil docs list|check|search`); never run `anvil docs verify` (human review only).
2. **MCP Tools (`anvil mcp`, configured in `.cursor/mcp.json`)**:
   - Code Graph: `graph_outline`, `graph_read_symbol` (read outlines/symbols before large files), `graph_query`, `graph_impact`, `graph_origin`, `graph_path`, `graph_explain`.
   - Memory & Tasks: `mem_search` -> `mem_timeline` -> `mem_get`, `mem_save`, `mem_context`; `task_start`, `task_update`, `task_done`, `task_resume`, `task_record`, `task_skip`, `task_list`, `task_get`.
3. **Clean Code**:
   - Avoid `@ts-ignore`, `@ts-expect-error`, `eslint-disable`, `!important`, `# noqa`, `@Suppress`, `@phpstan-ignore`, `@psalm-suppress`, or `TODO`/`FIXME`/`HACK` without `anvil-allow(<rule>): <reason> [<ref>]`.
   - Avoid pure barrel re-exports (`index.ts`) and unrequested root files outside `root_allowlist`.
4. **Skills**: Load workflows from `.cursor/skills/` (`subagent-routing`, `visual-qa`, `ratchet-review`, `memory-hygiene`, `living-docs`).
5. **Task Lifecycle & Memory (1 Task at a Time)**: Finish one task cleanly before starting the next: **Analyze -> `task_start` -> Implement -> `task_update` (checkpoint before tests) -> Verify (`anvil check`) -> `task_done` -> `mem_save` for non-obvious fixes**. On restart, run `task_resume` first.
6. **Zero Unverified Assumptions (Evidence First)**: Verify runtime permissions, auth/session state, DOM/Shadow DOM structure, and config facts before acting.
<!-- anvil:managed:end -->
