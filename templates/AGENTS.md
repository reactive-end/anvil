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
   - `anvil tasks start|update|done|build` — Conflict-free team task ledger (`.harness/tasks/` -> `tasks.json`).
   - `anvil ratchet log|list|review|test|promote` — Graduate recurring mistakes into deterministic rules with human approval.
   - `anvil deps` — Audit npm/PyPI/pub.dev/Maven/Cargo dependencies for deprecation or >24mo staleness.
   - `anvil qa run|onboard|eval-web|scan-mobile` — Objective DOM/View-Tree checks + before/after screenshots.
   - `anvil doctor` — Verify repo config, strict TS/lint setup, and adapter health.
2. **MCP Tools (`anvil mcp`, configured in `.agents/mcp_config.json`)**:
   - Code Graph: `graph_outline`, `graph_read_symbol` (read outlines/symbols before large files), `graph_query`, `graph_impact`, `graph_origin`, `graph_path`, `graph_explain`.
   - Memory (Progressive Disclosure): `mem_search` -> `mem_timeline` -> `mem_get`, `mem_save`, `mem_context`.
3. **Token Guard & Clean Code**:
   - `PreToolUse` guards against reading lockfiles, minified bundles, or huge files whole — use `graph_outline`/`graph_read_symbol` or bounded line ranges.
   - Never add `@ts-ignore`, `@ts-expect-error`, `eslint-disable`, `!important`, `# noqa`, `@Suppress`, or `TODO`/`FIXME`/`HACK` without an approved `anvil-allow(<rule>): <reason> [<ref>]`.
   - Avoid pure barrel re-exports (`index.ts`) and unrequested root files outside `root_allowlist`.
4. **Skills**: Load task-specific workflows from `.agents/skills/` (`subagent-routing`, `visual-qa`, `ratchet-review`, `memory-hygiene`).
5. **Automatic Task Tracking (Mandatory)**: Tasks are auto-created by hooks on user prompts; before concluding any code-editing turn, ALWAYS record a concise summary with `anvil tasks done <ID> "<summary>"` (or `anvil tasks update <ID> "<summary>"` if still in progress).
<!-- anvil:managed:end -->
