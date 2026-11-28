---
name: improve-prompt
description: When a request is vague or the user asks to improve it, turn it into a structured prompt with `anvil prompt improve`, get the user's confirmation, then do the work.
---

# Improve a Rough Request

Use this when the request has no clear target or success criteria, says "everything" or "the whole app", or the user asks you to improve, structure, or clarify their prompt.

1. Run `anvil prompt improve "<the user's exact text>"`. Add `--json` if you need to read it programmatically.
2. Show the user the result as is: Goal, Context, Constraints, Acceptance criteria, Verification, Open questions.
3. Ask the user to confirm or edit it. Fill the `<describe ...>` placeholders with them and answer each open question together. Do not guess the answers.
4. Only after the user confirms, work from the confirmed version. Run the commands under Verification before you say it is done.

`anvil prompt improve` works whatever the prompt assist mode is. The automatic context line is off by default; the user turns it on for every project with `anvil prompt assist context` (or `coach`) and off with `anvil prompt assist off`. Do not change that setting yourself.

Never edit files before step 3 is answered. If the user says to just proceed, state your assumptions in one sentence and go ahead.
