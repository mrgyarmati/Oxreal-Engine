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

## Amendment (2026-11-20, with `oxreal-hierarchy`)

This amends the Decision above. The "(proposed)" in its heading predates acceptance: the
Decision is Accepted.

1. **The authoring hierarchy is ECS data, not a separate store.** The Context asked for a
   hierarchy with "different requirements from a cache-optimized runtime store": stable order,
   parent links, local transforms. ECS components meet them:
   - an immutable `ChildOf` link on the child;
   - an ordered, read-only `Children` list on the parent, kept consistent by ECS hooks
     (ADR-0014 §3 and §4);
   - a `LocalTransform`.

   One source of truth removes the sync points the Consequences warned about. What remains to
   test is the hooks, which randomized invariant tests exercise. Stable object IDs for saved
   scenes remain separate, as decided above.
2. **When a parent is despawned, each link decides** (ADR-0014, §3 amendment, item 4). The
   child is detached and keeps its place in the world (the default, as in Unreal), or it's
   despawned first (opt-in, for the parts of one object).
3. **Attaching and detaching take an explicit rule:** keep the local transform, or keep the
   place in the world. There's no default. The editor's attach and detach will always keep the
   place in the world, as Unreal's do.
4. **World transforms** (`WorldTransform`, affine, so non-uniform scale stays exact) are
   computed by the `propagate transforms` system in `PostUpdate`, after gameplay and physics
   and before `Extract`, as decided above. Between gameplay changes and the next propagation,
   they're stale. Code that needs a current world placement mid-frame computes it from the local
   transforms.

Implementation details and measurements are in ADR-0014's notes on relations and the hierarchy.
