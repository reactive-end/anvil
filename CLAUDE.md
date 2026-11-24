<!-- anvil:managed:start -->

# CLAUDE.md — Claude Code Thin Adapter

> Single source of truth: `.harness/` (`config.json`, `rules/`, `candidates/`, `skills/`, `memory/chunks/`, `tasks/`).
> Synced automatically via `anvil sync` (`.claude/settings.json`, `.mcp.json`, `.claude/skills/`).

## Core Discipline

1. **Deterministic Commands (`anvil`)**:
   - `anvil check` — Tiered verification (`Tier 0` AST, `Tier 1` impact + typecheck + targeted tests, `Tier 2` visual QA & deps).
   - `anvil run -- <cmd>` — Compressed build/test/lint output (1-line PASS; <= 50-line failure; raw log in `.harness/.state/logs/`).
   - `anvil impact <files> --origin <target>` — Reverse-BFS blast-radius via `anvil-graph`; fix the origin, not downstream symptoms.
   - `anvil diff` / `anvil grep` — Token-compressed diff and symbol-aware code search.
   - `anvil daemon start|stop|status` & `anvil stats` — Warm background daemon and token/check telemetry.
   - `anvil circuit status|reset` — Circuit breaker trips at `N=3` identical failures; inspect root cause before resetting.
   - `anvil tasks record|start|update|done|skip|build` — LLM-curated, conflict-free team task ledger (`.harness/tasks/` -> `tasks.json`).
   - `anvil ratchet log|list|review|test|promote` — Governed rule graduation (triple filter + human approval).
   - `anvil deps` — Tier 2 npm/PyPI/pub.dev/Maven/Cargo/Composer audit (deprecated / >24mo stale packages).
   - `anvil qa run|onboard|eval-web|scan-mobile` — Hybrid DOM/View-Tree programmatic checks + before/after screenshots.
   - `anvil doctor` — Audit repository health and adapter wiring.
2. **Unified MCP Server (`anvil mcp`, registered in `.mcp.json`)**:
   - Graph: `graph_outline`, `graph_read_symbol` (read outlines/symbols before large files), `graph_query`, `graph_impact`, `graph_origin`, `graph_path`, `graph_explain`.
   - Memory & Curated Tasks: `mem_search` -> `mem_timeline` -> `mem_get`, `mem_save`, `mem_context`; `task_record`, `task_done`, `task_skip`, `task_list`.
3. **Token Guard & Zero Escape Hatches**:
   - `PreToolUse` guards against reading lockfiles, minified bundles, or huge files whole — use `graph_outline`/`graph_read_symbol` or bounded line ranges.
   - Forbidden unless structured `anvil-allow(<rule>): <reason> [<ref>]`: `@ts-ignore`, `@ts-expect-error`, `eslint-disable`, `!important`, `# noqa`, `@Suppress`, `@phpstan-ignore`, `@psalm-suppress`, `TODO`/`FIXME`/`HACK`.
   - Avoid pure barrel files (`index.ts`) and unapproved root files.
4. **Skills**: Follow `.claude/skills/` (`subagent-routing`, `visual-qa`, `ratchet-review`, `memory-hygiene`).
5. **LLM-Curated Task Governance (Strict Anti-Noise Bias)**: NEVER turn status questions (*"¿Cómo vamos con...?"*), explanations, git commits, or small 1-file tweaks (`<= 12` lines) into tasks, and NEVER paste raw user chat as a task title. For substantive features/refactors/bugfixes, synthesize a clean 3-18 word architectural title (e.g. `"Reorganización visual del módulo X"`) via MCP `task_record` (or `anvil tasks record "<Title>" "<Summary>"`); append follow-ups to the existing task via `task_done`, or call `task_skip` (`anvil tasks skip "<reason>"`) for minor chores.
6. **Zero Unverified Assumptions (Evidence First)**: Never assume runtime permissions, auth/session state, DOM structure (e.g. Shadow DOM vs light DOM), or config constraints without gathering runtime/source facts first. Always verify permission grant state after window focus and handle denial with an explicit settings/UI fallback (never empty `RequestPermission` callbacks or `adb pm grant` shortcuts).
<!-- anvil:managed:end -->
