# ADR-0007: Unreal Engine source is a UX reference only

- **Status:** Accepted
- **Date:** 2026-11-18
- **Related:** PRD §6, brief §4, §15

## Context

The editor should feel familiar to Unreal developers, and contributors may have a local Unreal
Engine source checkout for reference. Each contributor records its path in the git-ignored
`.claude/unreal-source.local.txt`. Unreal source is licensed under the Epic Games EULA, which is
incompatible with redistributing derived code in an MIT/Apache-2.0 project.

## Decision

- Unreal source may be consulted to understand **observable editor behaviour and terminology**:
  layouts, panel names, shortcuts, workflows and user-facing concepts.
- No code, shaders, comments, data files, assets or closely paraphrased algorithms from Unreal may
  enter this repository. Implementations are designed independently.
- Research output is written as behavioural notes (what the user sees and does), not as code or
  pseudo-code of Unreal internals. Use the `unreal-ux-researcher` agent.
- We don't promise compatibility with Unreal project files or formats.

## Consequences

- Clean-room discipline: reviewers should reject changes that mirror Unreal internals.
- Familiarity is achieved through UX, not through shared implementation.
