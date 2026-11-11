# Execution plans

A plan is a living document for one piece of non-trivial work (a milestone item, a feature, a
refactor). It lets a human or an agent pick up work mid-way without re-deriving context.

## Rules

- One file per piece of work: `YYYY-MM-DD-short-title.md`, created from the [template](TEMPLATE.md).
- Write the plan **before** coding; update the progress log and checkboxes **while** working.
- Record surprises and decisions in the plan; promote architectural ones to an [ADR](../adr/README.md).
- When done: set `Status: Done`, link the PR, tick the item in [roadmap.md](../roadmap.md).
  Keep finished plans — they are the project's memory.

## Index

| Plan | Status |
|---|---|
| [2026-11-06 — M0 skeleton](2026-11-06-m0-skeleton.md) | Done |
| [2026-11-06 — M1 connect & inventory](2026-11-06-m1-connect-inventory.md) | Done |
