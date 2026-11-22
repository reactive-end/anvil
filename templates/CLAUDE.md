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
   - `anvil tasks start|update|done|build` — Conflict-free team task ledger (`.harness/tasks/` -> `tasks.json`).
   - `anvil ratchet log|list|review|test|promote` — Governed rule graduation (triple filter + human approval).
   - `anvil deps` — Tier 2 npm/PyPI/pub.dev/Maven/Cargo audit (deprecated / >24mo stale packages).
   - `anvil qa run|onboard|eval-web|scan-mobile` — Hybrid DOM/View-Tree programmatic checks + before/after screenshots.
   - `anvil doctor` — Audit repository health and adapter wiring.
2. **Unified MCP Server (`anvil mcp`, registered in `.mcp.json`)**:
   - Graph: `graph_outline`, `graph_read_symbol` (read outlines/symbols before large files), `graph_query`, `graph_impact`, `graph_origin`, `graph_path`, `graph_explain`.
   - Memory (Progressive Disclosure): `mem_search` -> `mem_timeline` -> `mem_get`, `mem_save`, `mem_context`.
3. **Token Guard & Zero Escape Hatches**:
   - `PreToolUse` guards against reading lockfiles, minified bundles, or huge files whole — use `graph_outline`/`graph_read_symbol` or bounded line ranges.
   - Forbidden unless structured `anvil-allow(<rule>): <reason> [<ref>]`: `@ts-ignore`, `@ts-expect-error`, `eslint-disable`, `!important`, `# noqa`, `@Suppress`, `TODO`/`FIXME`/`HACK`.
   - Avoid pure barrel files (`index.ts`) and unapproved root files.
4. **Skills**: Follow `.claude/skills/` (`subagent-routing`, `visual-qa`, `ratchet-review`, `memory-hygiene`).
5. **Automatic Task Tracking (Mandatory)**: Tasks are auto-created by hooks on user prompts; before concluding any code-editing turn, ALWAYS record a concise summary with `anvil tasks done <ID> "<summary>"` (or `anvil tasks update <ID> "<summary>"` if still in progress).
<!-- anvil:managed:end -->
