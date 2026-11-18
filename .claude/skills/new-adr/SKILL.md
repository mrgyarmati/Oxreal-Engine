---
name: new-adr
description: Create a new Architecture Decision Record in docs/decisions with the next number, from the project template, and register it in the ADR index and open-decisions list. Use when an architectural decision is made or proposed, when boundaries in TAD.md change, or when resolving an item from docs/development/open-decisions.md.
---

# Create an ADR

Argument: the decision title (and optionally an open-decision ID such as `D-04`).

1. List `docs/decisions/` and take the next free four-digit number. Never reuse or renumber.
2. Create `docs/decisions/NNNN-kebab-case-title.md` from `docs/decisions/0000-template.md`:
   - **Status:** `Proposed` (only the user accepts ADRs). Use today's date.
   - **Related:** PRD and TAD sections, other ADRs, and the D-xx ID.
   - Context, at least two real options with verified evidence (no invented APIs or benchmarks),
     the decision, and consequences including revisit triggers.
   - For substantial research, delegate to the `adr-author` agent instead of writing from memory.
3. Add a row to the table in `docs/decisions/README.md`.
4. If it resolves an open decision, update that row's Status and ADR columns in
   `docs/development/open-decisions.md`.
5. If it changes layers or allowed dependencies, update `docs/architecture/TAD.md` §2–§4 **and**
   `Layer::may_depend_on` in `tools/xtask/src/layers.rs` (with tests), then run `cargo xtask ci`.
6. If it supersedes an ADR, set the old ADR's status to `Superseded by ADR-NNNN`. Change nothing
   else in the old ADR.
