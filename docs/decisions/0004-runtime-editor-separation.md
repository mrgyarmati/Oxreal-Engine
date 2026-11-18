# ADR-0004: Runtime/editor separation and enforced crate layering

- **Status:** Accepted
- **Date:** 2026-11-18
- **Related:** TAD §2–§4, brief §5

## Context

Player builds must exclude editor code and dependencies. Without mechanical enforcement, a single
convenience import tends to erode the boundary.

## Decision

- Every workspace crate declares `[package.metadata.engine] layer = "<layer>"` (TAD §2).
- `cargo xtask layers` checks each normal and build dependency edge between workspace crates
  against the allowed matrix (TAD §4) and fails CI on a violation or a missing layer.
- Editor-only data such as editor metadata on components lives in editor crates or behind
  editor-side registries. It must not be added to runtime types behind a `cfg(feature = "editor")`
  unless an ADR allows it.
- `app-runtime` (player, server) can never reach `editor` crates.

## Consequences

- Some glue needs an extra crate or trait to avoid a forbidden edge. That's intentional.
- Changing the matrix requires an ADR and a TAD update.
