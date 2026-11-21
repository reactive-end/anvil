---
name: memory-hygiene
description: Progressive disclosure memory workflow (mem_search -> mem_timeline -> mem_get), topic_key upserts, and strict boundaries vs anvil-graph and anvil ratchet.
---

# Memory Hygiene & Progressive Disclosure

`anvil-memory` provides local SQLite FTS5 + conflict-free Git chunks (`.harness/memory/chunks/`). Use it with strict discipline to minimize token usage and prevent stale prose.

## 1. Progressive Disclosure (Read Path)

Always load memory in three cheap steps — **never** bulk-dump memories into context:
1. `mem_search(query, kind?, limit?)` — Returns compact hits (title, `topic_key`, `kind`, date, freshness state) for a few tokens.
2. `mem_timeline(id | topic_key)` — Fetches compact chronological neighborhood only if evolution history matters.
3. `mem_get(id)` — Loads the full observation body **only** for hits you actively need.

If an observation is marked `needs_verification` or `stale` (flagged automatically via `anvil-graph` when linked files/symbols change), verify the current code before trusting it.

## 2. Strict Boundary (What Goes Where)

| Content | Correct System | Never Put In |
| :--- | :--- | :--- |
| Code structure, symbols, imports, JSX props, call paths | **`anvil-graph`** (`graph_*`) | `anvil-memory` |
| Repetitive coding mistakes & prohibitions | **`anvil ratchet`** (`.harness/rules/`) | `anvil-memory` |
| **Why** decisions were made (ADRs), env/lib **gotchas**, **domain** rules, **session** continuity | **`anvil-memory`** (`mem_save`) | Prosaic `AGENTS.md` / `CLAUDE.md` |

Valid `kind` values ONLY: `decision` | `gotcha` | `domain` | `session` (TTL-decayed).

## 3. `topic_key` Upserts & Privacy

- Pass a stable `topic_key` (e.g., `auth/jwt-refresh-race`, `build/expo-metro-cache`) to `mem_save` so evolving knowledge updates the existing record cleanly instead of creating duplicates.
- Always link `related_files` / `related_symbols` so `anvil-graph` can flag staleness automatically.
- Wrap sensitive snippets in `<private>...</private>`; secrets/tokens are automatically redacted before persistence.
