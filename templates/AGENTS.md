<!-- anvil:managed:start -->

# AGENTS.md — Antigravity Thin Adapter

> **Availability Guard**: Only apply this `anvil:managed` block if the `anvil` binary is on `PATH` (or the `anvil` MCP server is active). If `anvil` is not installed in this environment, **skip this entire block**—do NOT invoke `anvil` commands or treat `.harness/` as an active workflow/source of truth; use the repository's native tools instead.
> Single source of truth (when `anvil` is available): `.harness/` (`config.json`, `rules/`, `candidates/`, `skills/`, `memory/chunks/`, `tasks/`). Synced via `anvil sync`; do not duplicate docs here.

## Core Discipline

1. **Use `anvil` CLI & MCP** instead of manual guesswork:
   - `anvil check` — Tiered verification (`Tier 0` AST, `Tier 1` impact + typecheck + targeted tests, `Tier 2` visual QA & deps).
   - `anvil run -- <cmd>` — Token-compressed runner (1-line PASS; <= 50-line structured failure; raw log in `.harness/.state/logs/`).
   - `anvil impact <files> --origin <target>` — Blast-radius via `anvil-graph`; fix the true origin, never patch downstream.
   - `anvil diff` — Token-compressed diff. `anvil grep <pattern> [paths] [-i -F -w -g <glob> -C <n> -l]` — fast gitignore-aware code search grouped by file, with each match annotated with its enclosing symbol when a graph exists (`anvil graph build`); also exposed as the MCP `grep` tool.
   - `anvil daemon start|stop|status` & `anvil stats` — Warm background daemon and token/check telemetry.
   - `anvil circuit status|reset` — Stops blind retries after 3 identical failures (`N=3`); escalate and inspect root cause.
   - `anvil tasks start|update|done|resume|verify|record|skip|build` & `anvil mem save|search` — Crash-safe tasks (`tasks/<id>/progress.md` -> `tasks.json`) & persistent memory.
   - `anvil ratchet log|list|review|test|promote` — Graduate recurring mistakes into deterministic rules with human approval.
   - `anvil deps` — Audit npm/PyPI/pub.dev/Maven/Cargo/Composer dependencies for deprecation or >24mo staleness.
   - `anvil qa run|onboard|eval-web|scan-mobile` — Objective DOM/View-Tree checks + before/after screenshots.
   - `anvil doctor` — Verify repo config, strict TS/lint setup, and adapter health; `anvil prereqs check` — verify the machine has the tools, versions, env files and MCP servers the project needs. Living docs (OKF) in `.harness/docs/<project>/`: read `index.md` and the module's concept before touching a module; after behaviour changes update the concept and run `anvil docs refresh <id>` (`anvil docs list|check|search`); never run `anvil docs verify` (human review only).
2. **MCP Tools (`anvil mcp`, configured in `.agents/mcp_config.json`)**:
   - Graph: **Lookup order: graph -> docs -> code.** `search` first (code graph, then `.harness/docs/<project>/` concepts, then grep only if needed; CLI `anvil search <query> [--code]`), or `graph_query`/`graph_outline`/`graph_read_symbol` (read symbols, not whole files); `grep` last. `PreToolUse` answers the first code search of a turn with graph/docs results (`search.cascade`: enforce|advise|off). Also `graph_outline`, `graph_read_symbol`, `graph_query`, `graph_impact`, `graph_origin`, `graph_path`, `graph_explain`.
   - Memory & Tasks: `mem_search` -> `mem_timeline` -> `mem_get`, `mem_save`, `mem_context`; `task_start`, `task_update`, `task_done`, `task_resume`, `task_record`, `task_skip`, `task_list`, `task_get`.
3. **Token Guard & Clean Code**:
   - `PreToolUse` guards against reading lockfiles, minified bundles, or huge files whole — use `graph_outline`/`graph_read_symbol` or bounded line ranges.
   - Never add `@ts-ignore`, `@ts-expect-error`, `eslint-disable`, `!important`, `# noqa`, `@Suppress`, `@phpstan-ignore`, `@psalm-suppress`, or `TODO`/`FIXME`/`HACK` without an approved `anvil-allow(<rule>): <reason> [<ref>]`.
   - Avoid pure barrel re-exports (`index.ts`) and unrequested root files outside `root_allowlist`.
4. **Skills**: Load task-specific workflows from `.agents/skills/` (`subagent-routing`, `visual-qa`, `ratchet-review`, `memory-hygiene`, `living-docs`).
5. **Task Lifecycle, Crash Recovery & Memory (Strictly 1 Task at a Time)**: You decide when work warrants a task vs `task_skip`. For backlogs/multi-task requests, finish one task's full loop before touching the next (NEVER batch edits across tasks or chain `update`+`done` at the end): **Analyze -> `task_start`/`anvil tasks start <ID|title>` -> Implement code -> `task_update`/`anvil tasks update <ID> "..." --done ... --next ...` (immediately after code edits, before tests) -> Verify (`anvil tasks verify <ID> --record` / `anvil check`) -> `task_done`/`anvil tasks done <ID> "..."` -> if you fixed a non-obvious bug, gotcha, or architecture decision, save it with `mem_save`/`anvil mem save`**. On restart/crash, run `task_resume`/`anvil tasks resume` first.
6. **Zero Unverified Assumptions (Evidence First)**: Never assume runtime permissions, auth/session state, DOM structure (e.g. Shadow DOM vs light DOM), or config constraints without gathering runtime/source facts first. Always verify permission grant state after window focus and handle denial with an explicit settings/UI fallback (never empty `RequestPermission` callbacks or `adb pm grant` shortcuts).
<!-- anvil:managed:end -->
