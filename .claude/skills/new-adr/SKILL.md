---
name: new-adr
description: Draft a new Architecture Decision Record in docs/adr/. Use when a change introduces, reverses or supersedes an architectural decision, or when the user asks to "record a decision" / "write an ADR".
---

# New ADR

1. Read `docs/adr/README.md` and list existing ADRs. The new number is the highest number + 1, zero-padded to 4 digits.
2. Check whether an existing ADR already covers the topic. If the new decision contradicts an **Accepted** ADR, the new ADR supersedes it: do not rewrite the old one, only set its status line to `Superseded by ADR-NNNN`.
3. Copy `docs/adr/0000-template.md` to `docs/adr/NNNN-kebab-title.md` and fill in Context, Decision, Consequences. Link facts with dates and sources.
4. Status is always `Proposed` — only a human maintainer sets `Accepted`.
5. Add a row to the index table in `docs/adr/README.md`.
6. Update any design doc that the decision affects, and link the ADR from it.
