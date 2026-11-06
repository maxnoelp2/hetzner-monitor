---
name: new-plan
description: Create an execution plan in docs/plans/ before starting non-trivial work (a milestone item, feature or refactor). Use when starting multi-step implementation work or when the user asks to plan a task.
---

# New execution plan

1. Read `docs/roadmap.md`, `ARCHITECTURE.md` and the design docs and ADRs relevant to the task. Verify facts against the code before writing.
2. Copy `docs/plans/TEMPLATE.md` to `docs/plans/YYYY-MM-DD-short-title.md` (today's date).
3. Fill in Goal, Acceptance criteria (observable and testable), Context (what you read), Approach (ordered steps), Out of scope, Risks.
4. Add the plan to the index in `docs/plans/README.md` with status `Draft`.
5. Show the plan to the user and wait for approval before implementing.
6. While working: tick steps, append to the Progress log, record surprises. On completion set `Status: Done`, link the PR and tick the roadmap item.
