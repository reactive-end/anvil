---
name: ratchet-review
description: Governed retrospective lint rule graduation workflow using triple filter and mandatory human approval.
---

# Governed Lint Ratchet Workflow

Prevent linter fatigue and false positives by never adding ad-hoc lint rules mid-feature.

## 1. Capture Candidates Silently During Work

- When an agent makes a recurring mistake or violates a project invariant not yet enforced by Tier 0/linters, record it via `anvil ratchet log`:
  - Stored in `.harness/candidates/<id>.json`.
  - Non-blocking during feature implementation; does not pollute PR diffs or linter configs.

## 2. Retrospective Evaluation (`anvil ratchet review`)

During task retrospective or dedicated cleanup, inspect pending candidates with `anvil ratchet review` and apply the **Triple Filter**:

1. **Recurrence or Impact**: Recurrence count `>= 2` across sessions/tasks **OR** high severity impact (security, runtime crash, broken contract).
2. **Deterministic & Fast**: Check is 100% deterministic (AST/regex/config) and executes in **`< 200 ms`**.
3. **False-Positive Validation**: Run `anvil ratchet test <candidate-id>` against the entire repository to confirm zero false positives on existing valid code.

## 3. Human-Approved Graduation (`anvil ratchet promote`)

- Present passing candidates with their triple-filter evidence to the user.
- **Only after explicit human approval**, run `anvil ratchet promote <candidate-id>` to graduate the rule into `.harness/rules/`.
- Reject or refine candidates that fail any part of the triple filter.
