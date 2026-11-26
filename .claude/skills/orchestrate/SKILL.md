---
name: orchestrate
description: Work in orchestrator/executor mode in the current session - plan, decide, and review here while anvil-executor (sonnet, high effort) carries out the work. Use when the user asks to orchestrate or types /orchestrate.
---
<!-- anvil:managed:agent -->

# Orchestrate

For the rest of this conversation act as the orchestrator for: $ARGUMENTS

The session model is the orchestrator (configured: `opus` at `high` effort). If the session runs on a different model, say so in one line and continue; only the user can change it in the model selector.

1. Plan and decide here. Do not edit files yourself except for trivial one- or two-line fixes.
2. Delegate execution with the Agent tool using `subagent_type: anvil-executor`, without `model` or `effort` overrides. Each brief is self-contained: goal, a `SCOPE: path, path` line with what it may edit, files off limits, signatures or patterns to follow, constraints, and the command that proves it is done (`anvil check --scope <its SCOPE paths>`).
3. Start several executors in one message when their scopes share no file; give shared files to one of them or to a final step. Each executor edits only its own latest brief's scope, and a brief that overlaps a running executor is refused. Send follow-ups to the executor that already holds the context.
4. Review every result yourself: read the task's `review_packet` against the plan and its checklist, then `anvil check --scope <paths>` per executor and a plain `anvil check` once all are done. Return concrete corrections until it passes. When work that passed `anvil check` still has a real problem you send back, record it with `review_log` (category, severity, file); never for style preferences. When nothing is left to fix, approve with `review_approve`; in orchestrator mode `task_done` is refused until the current change is approved.
5. Keep the normal task lifecycle (`task_start`, `task_update`, `task_done`) in this session, one task at a time.
