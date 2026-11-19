# ADR-0005: Engine-owned ECS and separate authoring hierarchy

- **Status:** Accepted (2026-11-19, together with ADR-0014, which decides storage, change detection and scheduling)
- **Date:** 2026-11-18
- **Related:** TAD §5.1, brief §5; open decisions D-01, D-02

## Context

The ECS is the runtime's central data model. Adopting an external ECS (for example `bevy_ecs`)
would make it our permanent public foundation, which the brief rules out. Editors also need a
stable, ordered parent/child authoring hierarchy, and that has different requirements from a
cache-optimized runtime store.

## Decision (proposed)

- The engine owns its ECS API and implementation in `runtime/ecs`. Entities are generational
  indices, and components are plain Rust types registered with reflection.
- The authoring hierarchy lives in `runtime/hierarchy`. It stores ordered children, parent links
  and local transforms.
- The hierarchy is authoritative for parentage. World transforms are computed by an explicit
  propagation system scheduled after gameplay and physics write-back and before render extraction.
- Saved scenes serialize the authoring view (hierarchy and reflected components) with stable
  per-scene object IDs. They never serialize ECS storage layout.
- Still open: archetype vs sparse-set vs hybrid storage, change detection, and the parallel
  scheduling model (D-01). These will be decided with benchmarks against the M1 and M2 workloads.

## Consequences

- Significant implementation effort, which reflection integration and editor needs justify.
- Hierarchy and ECS must be kept consistent, so the sync points must be tested explicitly.
