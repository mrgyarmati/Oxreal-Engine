# ADR-0014: ECS architecture and storage

- **Status:** Accepted
- **Date:** 2026-11-19
- **Related:** ADR-0005 (engine-owned ECS and hierarchy, accepted with this ADR), ADR-0008 §3 (gameplay framework requirements), ADR-0010 (diagnostics), TAD §5.1 and §5.8 (C ABI constraints), D-01, D-02 (reflection); prior art: [Cordite](../development/prior-art.md)

## Context

ADR-0005 commits Oxreal to an engine-owned ECS with a separate authoring hierarchy, and leaves
storage, change detection and scheduling open (D-01). Since then, ADR-0008 §3 has listed what the
gameplay framework needs from the ECS, and TAD §5.8 requires gameplay-facing APIs that a future
C ABI can expose.

The ECS must support:

1. **Relations** (possession, attachment, ownership), kept consistent when entities despawn.
2. **Lifecycle hooks** (add, insert, replace and remove of a component; spawn and despawn).
3. **Deferred commands** applied at defined sync points.
4. **Stages with ordering** (tick groups).
5. **Reflection-driven access by component ID** for the Details panel, serialization, templates,
   replication, scripting and the C ABI.
6. **Cheap has-component checks**, which serve as role tests and casts.

On top of those, the engine itself needs:
- **change detection** for incremental render extraction, replication and editor refresh
- fast bulk iteration for transform propagation, render extraction and physics sync
- tolerable structural-change cost for editing and gameplay

### Prior art studied (designs only, no dependencies, per ADR-0001)

- **Cordite (EnTT, sparse sets).** It worked in production at Cordite's scale.
  - **Good:** its `World` wrapper added stable IDs, names, an ordered hierarchy and world
    transforms. The `World` has a stable address. Children are destroyed before parents, and an
    entity keeps its identity, transform and hierarchy place while its components are destroyed.
  - **Limiting:** the EnTT registry was exposed for iteration, so every system depended on
    EnTT's storage model.
- **Archetype designs** (Bevy's table storage, flecs, Unity DOTS, hecs) store entities with the
  same component set together in columns. Multi-component iteration is dense and cache-friendly,
  while adding or removing a component moves the entity between tables.
- **Sparse-set designs** (EnTT, shipyard) keep one dense array per component type. Add and
  remove are O(1). Single-component iteration is dense, but multi-component queries probe the
  other sets (EnTT's *groups* recover density for chosen sets).
- **Hybrids:** Bevy offers table (default) or sparse-set storage per component type, and flecs v4
  added sparse components. Relations: flecs models relations as (relation, target) pairs inside
  archetypes, which is powerful but fragments archetypes. Bevy implements parent/child as
  components with an automatically maintained inverse.

## Options considered

- **A. Sparse sets only (Cordite and EnTT model).** The simplest storage, with O(1) structural
  changes, proven in Cordite. Multi-component queries (the engine's hottest loops: transforms,
  extraction, physics sync) are less cache-friendly. Groups would add API complexity.
- **B. Archetype tables only.** Dense multi-component iteration and efficient per-table change
  detection. Every add or remove moves the entity's whole row, which is costly for frequently
  toggled tag components.
- **C. Hybrid: archetype tables by default, with sparse-set storage as an opt-in per component
  type.** The best fit for both bulk queries and frequent toggling, at the cost of two storage
  paths.
- **Expose storage (like Cordite's registry) versus keep it private.** Exposure makes the
  storage choice permanent. Keeping it private lets benchmarks change it later without breaking
  users.

## Decision

### 1. Storage is private; the API is storage-independent
The public `oxreal-ecs` API (entities, components, queries, commands, hooks and resources)
exposes **no storage types**. Storage can be changed or tuned, based on benchmark evidence,
without breaking engine, gameplay or plugin code. This is the main lesson from Cordite's
exposed registry.

### 2. Storage model: archetype tables, with sparse sets as a later opt-in (option C, staged)
- **Archetype tables are the default and the only storage in the first implementation (M1).**
  Each table stores one column per component type, contiguous and type-erased (layout, drop and
  clone through the component registry).
- **Sparse-set storage is an opt-in per component type** (`#[component(storage = sparse)]`). It
  is added **only when a benchmark or a real workload shows** frequent add and remove of that
  component dominating. It isn't built speculatively.
- Structural changes (spawn, despawn, add, remove) move entities between tables, with cached
  edges between archetypes, and are applied **only at sync points** through commands (§5) or by
  exclusive world access.

### 3. Entities, identity and relations
- `Entity` is a 64-bit generational handle (32-bit index, 32-bit generation), `Copy`,
  `#[repr(transparent)]`, and stable across the future C ABI. Stale handles fail lookups and
  never alias new entities.
- Entity handles are **runtime-only**. Persistent identity for scenes, prefabs, networking and
  references in files is a separate stable object ID component (ADR-0005), never the `Entity`
  value.
- **Relations are components that hold an `Entity`, with automatically maintained inverses**,
  for example `ChildOf(parent)` maintained alongside `Children`, and `Possesses(pawn)` alongside
  `ControlledBy(controller)`. Hooks keep both sides consistent, and despawning an entity
  resolves its relations by a per-relation policy (despawn the dependents, or clear the link).
  Pair-based archetypes in the flecs style are not used initially.
- The authoring hierarchy (ADR-0005) is built on the `ChildOf`/`Children` relation, with ordered
  children. Transform propagation is a scheduled system, as before.

### 4. Lifecycle hooks
- Per-component-type hooks: `on_add`, `on_insert`, `on_replace` and `on_remove`. They run
  synchronously as structural changes are applied, and they may only queue further changes
  through commands. They maintain engine invariants such as relation inverses, render-proxy
  registration and physics body creation.
- Despawn order, following Cordite's validated behaviour: **children before parents**, and while
  `on_remove` hooks run, the entity still has its identity, transform and hierarchy place.
- Gameplay-level events (`begin_play`, `end_play`, overlap and so on, ADR-0008) are built on
  hooks plus an engine event system. They aren't hooks themselves.

### 5. Commands, stages and scheduling
- **Commands** are deferred structural changes (spawn, despawn, insert, remove, run a closure
  with exclusive world access), recorded by systems and behaviours and **applied in order at
  sync points**. The application order within a stage is deterministic.
- **Stages** are the tick groups of ADR-0008, preceded by an explicit `Input` stage that this ADR
  adds:
  `Input → PrePhysics → DuringPhysics → PostPhysics → PostUpdate (transform propagation) →
  Extract`.
  - Ordinary gameplay logic runs in `PrePhysics`, as Unreal actors tick in `TG_PrePhysics` by
    default.
  - `Input` samples devices and runs input routing (contexts and consumption, a lesson from
    Cordite's R1) before any gameplay sees the frame.
  - Each stage ends with a sync point. Systems declare ordering
  constraints within a stage (`before` and `after`), and cycles are a startup error naming the
  systems involved.
- Systems **declare their data access** (components and resources, read or write). The first
  executor runs them **serially in a deterministic order**. A parallel executor that uses the
  declared access to avoid conflicts comes once profiling justifies it, with no API change,
  because access is declared from day one.

### 6. Change detection
- Each component value records **added** and **changed** ticks (32-bit, with wraparound handled
  by periodic clamping). Queries can filter by added or changed since the system last ran.
- Mutable access marks a value changed only when it's actually dereferenced mutably.

### 7. Queries: static and dynamic
- **Static, typed queries** for Rust code, for example a query over `(&mut Transform,
  &Velocity)` filtered by `With<Pawn>` and `Changed<Transform>`.
- **Dynamic queries by `ComponentId`**, using reflection metadata (D-02). They serve the editor,
  serialization, scripting and the future C ABI (TAD §5.8). Both run on the same storage and
  matching logic.
- `has::<T>()` and `has_id(id)` are constant-time per entity (ADR-0008 §3.6).

### 8. Resources and events
- **Resources** are typed singletons in the `World` (time, input state, settings), accessed
  through the same declared-access system, so the scheduler sees them.
- **Events** are typed, double-buffered per frame, and readable by every system that declares
  them. The detailed event and observer design follows with the gameplay framework (ADR-0008).

### 9. World ownership and threading
- A `World` has a **stable address** for its lifetime (Cordite's validated choice). Loading a
  scene clears and refills it rather than replacing it, so systems and hooks keep valid
  references across scene changes.
- `World` is `Send` but not shared mutably across threads. Parallelism comes through the
  scheduler's access-checked execution, not locks inside storage.

### 10. Safety, testing and performance evidence
- Type-erased columns need `unsafe`. It's confined to the storage module of `oxreal-ecs`, every
  block carries a `// SAFETY:` justification, the code is reviewed by the `unsafe-auditor`
  agent, and it's tested under **Miri** in CI (a pinned nightly used only for that job).
- Property-based tests check structural invariants: entity and relation consistency, and that
  hooks fire exactly once per change.
- An **ECS benchmark suite** (`benchmarks/ecs`) ships with the first implementation. It covers:
  - spawn and despawn of 100k entities
  - multi-component iteration over 100k entities with 1, 10 and 100 archetypes
  - transform propagation for 100k entities in a hierarchy four levels deep
  - add and remove churn of a tag on 10k entities per frame
  - random access by `Entity`

  Results are recorded with hardware details (PRD §9). This suite is the evidence for adding
  sparse-set storage (§2) or changing storage later.

## Consequences

- One ECS that meets every ADR-0008 requirement and the C ABI constraints, with storage kept
  private so it can evolve on evidence instead of guesses.
- Archetype storage gives dense iteration for the engine's hottest loops. Tag-toggling cost is
  the known weak spot, with a defined, evidence-gated remedy (sparse-set opt-in).
- Significant implementation effort, delivered in slices with M1: entities, archetype tables,
  static queries, commands and stages. Then relations and hooks, which the hierarchy needs, then
  change detection and dynamic queries, which extraction and the editor need.
- `unsafe` storage code adds a Miri CI job and an audit burden, and these are deliberate costs.
- ADR-0005 is accepted with this ADR. ADR-0008 stays Proposed until the gameplay framework is
  built, but its §3 requirements are now commitments of the ECS.
- Revisit triggers: benchmark results contradicting the storage choice, relation queries needing
  pair-based archetypes, or parallel execution being blocked by the access model.
