---
name: subagent-routing
description: Subagent routing matrix and minimal context packet specification to avoid over-delegation and wasted tokens.
---

# Subagent Routing Matrix

Use this decision matrix before spawning any subagent or worker:

| Work Shape | Route | Rationale |
| :--- | :--- | :--- |
| Bounded mechanical refactor across **>= 3 files** with a closed pattern | **Delegate** (smaller/worker model) | Repetitive edits parallelize cleanly with low ambiguity. |
| Boilerplate or unit tests following an **existing template** | **Delegate** (smaller/worker model) | Pattern is fixed; verifier/tests catch drift. |
| Isolated file/module read & summary with explicit paths from `anvil-graph` | **Delegate** (smaller/worker model) | Keeps raw file noise out of the coordinator context. |
| Trivial **1–2 line edits** | **Do Inline** | Subagent startup and handoff overhead exceeds the edit cost. |
| Architecture/design decisions, shared contracts, or ambiguous multi-file debugging | **Forbid Delegation** | Requires full reasoning context and cross-cutting judgment. |

## Minimal Context Packet Specification (`anvil context pack`)

Every delegated task MUST receive a compact, self-contained context packet — **never** dump the entire repository or conversation:

1. **Ownership & Scope**: Exact list of target file paths (from `anvil-graph` / `anvil impact`) and explicit "do not touch" boundaries.
2. **Relevant Symbols & Edges**: Only the target signatures, caller/callee edges, or existing template file path.
3. **Linked Memories**: Specific `mem_get(id)` findings only if `mem_search` returned relevant `decision`/`gotcha` items.
4. **Done Criteria**: Exact deterministic command to verify (`anvil run -- <test_or_lint_cmd>` or `anvil check`).
