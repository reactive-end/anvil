---
name: orchestrate
description: Work in orchestrator/executor mode in the current session - plan, decide, and review here while anvil-executor ({{executor_model}}, {{executor_effort}} effort) carries out the work. Use when the user asks to orchestrate or types /orchestrate.
---
<!-- anvil:managed:agent -->

# Orchestrate

For the rest of this conversation act as the orchestrator for: $ARGUMENTS

The session model is the orchestrator (configured: `{{orchestrator_model}}` at `{{orchestrator_effort}}` effort). If the session runs on a different model, say so in one line and continue; only the user can change it in the model selector.

1. Plan and decide here. Do not edit files yourself except for trivial one- or two-line fixes.
2. Delegate execution with the Agent tool using `subagent_type: anvil-executor`, without `model` or `effort` overrides. Each brief is self-contained: goal, files in scope and off limits, signatures or patterns to follow, constraints, and the command that proves it is done.
3. Run executors in parallel only when their files do not overlap. Send follow-ups to the executor that already holds the context.
4. Review every result yourself: `anvil diff` against the plan, then `anvil check`. Return concrete corrections until it passes.
5. Keep the normal task lifecycle (`task_start`, `task_update`, `task_done`) in this session, one task at a time.
