<!-- anvil:managed:start -->

# AGENTS.md — Antigravity Thin Adapter

> Single source of truth: `.harness/` (`config.json`, `rules/`, `candidates/`, `skills/`, `memory/chunks/`, `tasks/`).
> Synced automatically via `anvil sync`. Do not duplicate docs here.

## Core Discipline

1. **Use `anvil` CLI & MCP** instead of manual guesswork:
   - `anvil check` — Tiered verification (`Tier 0` AST, `Tier 1` impact + typecheck + targeted tests, `Tier 2` visual QA & deps).
   - `anvil run -- <cmd>` — Token-compressed runner (1-line PASS; <= 50-line structured failure; raw log in `.harness/.state/logs/`).
   - `anvil impact <files> --origin <target>` — Blast-radius via `anvil-graph`; fix the true origin, never patch downstream.
   - `anvil diff` / `anvil grep` — Token-compressed diff and symbol-aware code search.
   - `anvil daemon start|stop|status` & `anvil stats` — Warm background daemon and token/check telemetry.
   - `anvil circuit status|reset` — Stops blind retries after 3 identical failures (`N=3`); escalate and inspect root cause.
   - `anvil tasks record|start|update|done|skip|build` — LLM-curated, conflict-free team task ledger (`.harness/tasks/` -> `tasks.json`).
   - `anvil ratchet log|list|review|test|promote` — Graduate recurring mistakes into deterministic rules with human approval.
   - `anvil deps` — Audit npm/PyPI/pub.dev/Maven/Cargo/Composer dependencies for deprecation or >24mo staleness.
   - `anvil qa run|onboard|eval-web|scan-mobile` — Objective DOM/View-Tree checks + before/after screenshots.
   - `anvil doctor` — Verify repo config, strict TS/lint setup, and adapter health.
2. **MCP Tools (`anvil mcp`, configured in `.agents/mcp_config.json`)**:
   - Code Graph: `graph_outline`, `graph_read_symbol` (read outlines/symbols before large files), `graph_query`, `graph_impact`, `graph_origin`, `graph_path`, `graph_explain`.
   - Memory & Curated Tasks: `mem_search` -> `mem_timeline` -> `mem_get`, `mem_save`, `mem_context`; `task_record`, `task_done`, `task_skip`, `task_list`.
3. **Token Guard & Clean Code**:
   - `PreToolUse` guards against reading lockfiles, minified bundles, or huge files whole — use `graph_outline`/`graph_read_symbol` or bounded line ranges.
   - Never add `@ts-ignore`, `@ts-expect-error`, `eslint-disable`, `!important`, `# noqa`, `@Suppress`, `@phpstan-ignore`, `@psalm-suppress`, or `TODO`/`FIXME`/`HACK` without an approved `anvil-allow(<rule>): <reason> [<ref>]`.
   - Avoid pure barrel re-exports (`index.ts`) and unrequested root files outside `root_allowlist`.
4. **Skills**: Load task-specific workflows from `.agents/skills/` (`subagent-routing`, `visual-qa`, `ratchet-review`, `memory-hygiene`).
5. **LLM-Curated Task Governance (Strict Anti-Noise Bias)**: NEVER turn status questions (_"¿Cómo vamos con...?"_), explanations, git commits, or small 1-file tweaks (`<= 12` lines) into tasks, and NEVER paste raw user chat as a task title. For substantive features/refactors/bugfixes, synthesize a clean 3-18 word architectural title (e.g. `"Reorganización visual del módulo X"`) via MCP `task_record` (or `anvil tasks record "<Title>" "<Summary>"`); append follow-ups to the existing task via `task_done`, or call `task_skip` (`anvil tasks skip "<reason>"`) for minor chores.
6. **Zero Unverified Assumptions (Evidence First)**: Never assume runtime permissions, auth/session state, DOM structure (e.g. Shadow DOM vs light DOM), or config constraints without gathering runtime/source facts first. Always verify permission grant state after window focus and handle denial with an explicit settings/UI fallback (never empty `RequestPermission` callbacks or `adb pm grant` shortcuts).
<!-- anvil:managed:end -->
