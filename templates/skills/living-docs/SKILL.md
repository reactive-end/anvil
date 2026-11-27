---
name: living-docs
description: Read, write, and refresh the living project documentation (OKF concepts under .harness/docs/), keep it in sync with code changes, and never record a human verification.
---

# Living Docs (Open Knowledge Format)

Project documentation lives in `.harness/docs/<project>/` as an OKF v0.3 bundle: Markdown files with YAML frontmatter. `index.md` lists the modules (read it first); `log.md` is the dated change history. Both are reserved and never concepts. A monorepo has one bundle per subrepo.

## 1. Read Before You Touch a Module

1. Read `.harness/docs/<project>/index.md`, then the concept of the module you are about to change (`<module>/overview.md`). Use `anvil docs search <query>` to find concepts and `anvil docs list` to see their status, trust tier, and whether they are stale.
2. A stale concept (its code changed since the doc was synced) may be wrong: check the code, then fix the doc.

## 2. Write a Concept

`anvil docs new <module> --paths <glob>...` creates `<module>/overview.md`; `anvil docs scaffold` creates one for every module that has none (from the code graph, so run `anvil graph build` first). Then replace each hint with real text:

- **Purpose**: why the module exists, in a few sentences.
- **Responsibilities**: what it owns and what it does not.
- **Public interface**: the contract callers rely on. Keep the generated outline block (below) and add prose around it.
- **Invariants**: what must always hold (ordering, idempotency, ownership, error semantics).
- **Dependencies**: what it needs from other modules and why.
- **Related**: links to neighbouring concepts (plain Markdown links, relative to the file or from the bundle root with a leading `/`).

Frontmatter you may edit: `title`, `description`, `tags`, `status` (`draft`, `stable`, `deprecated`), `x-anvil-paths` (repo-relative globs of the code the concept documents; each must match at least one file). Never write placeholder markers such as TODO or FIXME. Unknown frontmatter keys are preserved.

## 3. The Outline Block

Inside "Public interface", the text between `<!-- anvil:outline:start -->` and `<!-- anvil:outline:end -->` is generated from the code graph (exported symbols with kind and `file:line`). Never edit between the markers: `anvil docs refresh` regenerates it. Everything outside the markers is yours and is preserved byte for byte.

## 4. Refresh After Behaviour Changes

When a hook says a concept documents a file you edited:

1. Decide whether behaviour, contract, or invariants changed. If so, edit the concept's prose.
2. Run `anvil docs refresh <project>/<module>` (or `anvil docs refresh --stale`). It regenerates the outline, sets `generated` to now, records the current commit in `x-anvil-synced-commit`, and appends to `log.md`.
3. Run `anvil docs check` before finishing: errors (missing `type`, broken links, globs matching nothing, missing `okf_version`) must be fixed; stale warnings should be cleared.

## 5. Refresh vs Verify

- `refresh` is yours: it says "this doc now matches the code".
- `verify` records a **human** review (`verified: [{by: "human:<name>", at}]`) and lifts the concept to the human-reviewed trust tier. **Never run `anvil docs verify`**: the security guard asks the user, orchestrator mode refuses it for every agent, and a verification older than the latest `generated.at` no longer counts, so a refreshed doc needs a new human review.
