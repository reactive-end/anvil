<!-- anvil:managed:start -->
# CLAUDE.md — Claude Code Thin Adapter

> Single source of truth: `.harness/` (`config.json`, `rules/`, `candidates/`, `skills/`, `memory/chunks/`, `tasks/`).
> Synced automatically via `anvil sync` (`.claude/settings.json`, `.mcp.json`, `.claude/skills/`).

## Core Discipline
1. **Deterministic Commands (`anvil`)**:
   - `anvil check` — Tiered verification (`Tier 0` AST, `Tier 1` blast-radius + targeted typecheck/tests, `Tier 2` visual QA & deps).
   - `anvil run -- <cmd>` — Compressed build/test/lint output (1-line PASS; <= 50-line failure; raw log in `.harness/.state/logs/`).
   - `anvil impact <files> --origin` — Reverse-BFS blast-radius via `anvil-graph`; fix the origin, not downstream symptoms.
   - `anvil circuit status|reset` — Circuit breaker trips at `N=3` identical failures; inspect root cause before resetting.
   - `anvil tasks start|update|done|build` — Conflict-free team task ledger (`.harness/tasks/` -> `tasks.json`).
   - `anvil ratchet review|test|promote` — Governed rule graduation (triple filter + human approval).
   - `anvil deps` — Tier 2 registry audit (deprecated / >24mo stale packages).
   - `anvil qa` — Hybrid DOM/View-Tree programmatic checks + before/after screenshots.
   - `anvil doctor` — Audit repository health and adapter wiring.
2. **Unified MCP Server (`anvil mcp`, registered in `.mcp.json`)**:
   - Graph: `graph_query`, `graph_impact`, `graph_origin`, `graph_path`, `graph_explain`.
   - Memory (Progressive Disclosure): `mem_search` -> `mem_timeline` -> `mem_get`, `mem_save`, `mem_context`.
3. **Zero Escape Hatches & Hygiene**:
   - Forbidden unless structured `anvil-allow(<rule>): <reason> [<ref>]`: `@ts-ignore`, `@ts-expect-error`, `eslint-disable`, `!important`, `# noqa`, `@Suppress`, `TODO`/`FIXME`/`HACK`.
   - Avoid pure barrel files (`index.ts`) and unapproved root files.
4. **Skills**: Follow `.claude/skills/` (`subagent-routing`, `visual-qa`, `ratchet-review`, `memory-hygiene`).
<!-- anvil:managed:end -->
