# ADR-0001: Greenfield engine architecture

- **Status:** Accepted
- **Date:** 2026-11-18
- **Related:** PRD §2.1, brief §1, §16

## Context

The product aims for Unreal-class scope and wants to own its architecture: ECS, reflection,
rendering, asset pipeline and editor. Building on an existing engine (Bevy, Fyrox, Godot, Unreal)
would make that engine's architecture, release cadence and public API ours. In particular, a
dependency's ECS or reflection would become our public API.

## Options considered

- **Fork an existing engine (Bevy/Fyrox).** This is the fastest route to something on screen.
  The cost is inheriting core design decisions (ECS, reflection, renderer) that we would later
  need to replace, and divergence becomes painful.
- **Embed an engine as a backend.** This is the same problem with an extra abstraction layer.
- **Greenfield core, with focused libraries.** We own the architecture and use vetted libraries for
  well-bounded problems such as math, windowing, Vulkan bindings, physics, audio decoding and
  image decoding, hidden behind engine interfaces.

## Decision

Greenfield. We own the ECS, reflection, scheduler, RHI, renderer, asset database, editor framework
and gameplay framework. Third-party libraries are allowed for bounded problems, behind
engine-owned interfaces, after an audit (`docs/development/dependencies.md`). Bevy (including
`bevy_ecs` and `bevy_reflect`), Fyrox, Godot and Unreal are banned as dependencies in `deny.toml`.

## Consequences

- Much more upfront work, and time to first pixel is longer.
- Full control over the public API, the stability policy and the editor/runtime split.
- We must study the prior art (Bevy, Flecs, Our Machinery, Unreal UX) for ideas without importing it.
- Revisit trigger: none for the foundation itself. Individual library choices can be revisited.
