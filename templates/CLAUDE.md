<!-- anvil:managed:start -->

# CLAUDE.md — Claude Code Thin Adapter

> **Availability Guard**: Only apply this `anvil:managed` block if the `anvil` binary is on `PATH` (or the `anvil` MCP server is active). If `anvil` is not installed in this environment, **skip this entire block**—do NOT invoke `anvil` commands or treat `.harness/` as an active workflow/source of truth; use the repository's native tools instead.
> Single source of truth (when `anvil` is available): `.harness/` (`config.json`, `rules/`, `candidates/`, `skills/`, `memory/chunks/`, `tasks/`). Synced via `anvil sync` (`.claude/settings.json`, `.mcp.json`, `.claude/skills/`).

## Core Discipline

1. **Deterministic Commands (`anvil`)**:
   - `anvil check` — Tiered verification (`Tier 0` AST, `Tier 1` impact + typecheck + targeted tests, `Tier 2` visual QA & deps).
   - `anvil run -- <cmd>` — Compressed build/test/lint output (1-line PASS; <= 50-line failure; raw log in `.harness/.state/logs/`).
   - `anvil impact <files> --origin <target>` — Reverse-BFS blast-radius via `anvil-graph`; fix the origin, not downstream symptoms.
   - `anvil diff` — Token-compressed diff. `anvil grep <pattern> [paths] [-i -F -w -g <glob> -C <n> -l]` — fast gitignore-aware code search grouped by file, with each match annotated with its enclosing symbol when a graph exists (`anvil graph build`); also exposed as the MCP `grep` tool.
   - `anvil daemon start|stop|status` & `anvil stats` — Warm background daemon and token/check telemetry.
   - `anvil circuit status|reset` — Circuit breaker trips at `N=3` identical failures; inspect root cause before resetting.
   - `anvil tasks start|update|done|resume|verify|record|skip|build` & `anvil mem save|search` — Crash-safe tasks (`tasks/<id>/progress.md` -> `tasks.json`) & persistent memory.
   - `anvil ratchet log|list|review|test|promote` — Governed rule graduation (triple filter + human approval).
   - `anvil deps` — Tier 2 npm/PyPI/pub.dev/Maven/Cargo/Composer audit (deprecated / >24mo stale packages).
   - `anvil qa run|onboard|eval-web|scan-mobile` — Hybrid DOM/View-Tree programmatic checks + before/after screenshots.
   - `anvil doctor` — Audit repository health and adapter wiring. `anvil orchestrator setup|status|on|off` — planner/executor model pair (`/orchestrate`).
2. **Unified MCP Server (`anvil mcp`, registered in `.mcp.json`)**:
   - Graph: `graph_outline`, `graph_read_symbol` (read outlines/symbols before large files), `graph_query`, `graph_impact`, `graph_origin`, `graph_path`, `graph_explain`.
   - Memory & Tasks: `mem_search` -> `mem_timeline` -> `mem_get`, `mem_save`, `mem_context`; `task_start`, `task_update`, `task_done`, `task_resume`, `task_record`, `task_skip`, `task_list`, `task_get`.
3. **Token Guard & Zero Escape Hatches**:
   - `PreToolUse` guards against reading lockfiles, minified bundles, or huge files whole — use `graph_outline`/`graph_read_symbol` or bounded line ranges.
   - Forbidden unless structured `anvil-allow(<rule>): <reason> [<ref>]`: `@ts-ignore`, `@ts-expect-error`, `eslint-disable`, `!important`, `# noqa`, `@Suppress`, `@phpstan-ignore`, `@psalm-suppress`, `TODO`/`FIXME`/`HACK`.
   - Avoid pure barrel files (`index.ts`) and unapproved root files.
4. **Skills**: Follow `.claude/skills/` (`subagent-routing`, `visual-qa`, `ratchet-review`, `memory-hygiene`).
5. **Task Lifecycle, Crash Recovery & Memory (Strictly 1 Task at a Time)**: You decide when work warrants a task vs `task_skip`. For backlogs/multi-task requests, finish one task's full loop before touching the next (NEVER batch edits across tasks or chain `update`+`done` at the end): **Analyze -> `task_start`/`anvil tasks start <ID|title>` -> Implement code -> `task_update`/`anvil tasks update <ID> "..." --done ... --next ...` (immediately after code edits, before tests) -> Verify (`anvil tasks verify <ID> --record` / `anvil check`) -> `task_done`/`anvil tasks done <ID> "..."` -> if you fixed a non-obvious bug, gotcha, or architecture decision, save it with `mem_save`/`anvil mem save`**. On restart/crash, run `task_resume`/`anvil tasks resume` first.
6. **Zero Unverified Assumptions (Evidence First)**: Never assume runtime permissions, auth/session state, DOM structure (e.g. Shadow DOM vs light DOM), or config constraints without gathering runtime/source facts first. Always verify permission grant state after window focus and handle denial with an explicit settings/UI fallback (never empty `RequestPermission` callbacks or `adb pm grant` shortcuts).
<!-- anvil:managed:end -->
