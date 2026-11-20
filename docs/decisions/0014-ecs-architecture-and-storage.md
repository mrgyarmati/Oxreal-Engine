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

## Implementation notes (2026-11-20, first slice)

The first slice (`oxreal-ecs`) covers:
- entities, the component registry and archetype tables;
- bundles;
- typed queries with `With`/`Without` filters;
- resources.

Commands and stages, relations and hooks, and change detection and dynamic queries follow as
separate slices.

- **The columns are typed, not layout-erased, and have no `unsafe` code.** This deviates from §2
  and §10:
  - §2 describes type-erased columns, with layout, drop and clone through the component
    registry, and §10 expected them to need `unsafe`.
  - Here each column is a `Vec<T>` behind a small trait object, and the registry knows
    components only by Rust `TypeId`. There's no clone hook yet.
  - The trait is used only for whole-table operations: moving or dropping a row, and creating
    a column. Queries iterate the `Vec<T>` slices directly, so the dynamic dispatch happens once
    per archetype per query, not once per entity.
  - The crate is `#![forbid(unsafe_code)]`.

  The slice that adds **dynamic queries and components without a Rust type** must bring back
  layout-erased columns. Those are components defined by C# and C++ (TAD §5.8) or by scripts
  (ADR-0008 §3.5). That slice also brings:
  - `unsafe` code, with the Miri job and an `unsafe-auditor` review that §10 requires;
  - a registry path keyed on something other than `TypeId`.

  Storage is private (§1), so this changes no API.
- **Mutable access goes through `Mut<T>`, never a raw `&mut T`.** `&mut T` query items and
  `World::get_mut` yield `Mut<'w, T>`, a smart pointer used like `&mut T`. For now it passes
  writes straight through. It exists so §6's rule (a value is changed only when it's
  dereferenced mutably) can be added without a breaking change. A named iterator wraps each
  value, so iteration costs the same as over `&mut T`.
- **Query and bundle traits are sealed.** Callers name `QueryData`, `QueryFilter` and `Bundle`,
  but only `oxreal-ecs` implements them, so no internal storage type is public. Custom bundle
  types (a derive) come with reflection (D-02).
- **Queries are checked for aliasing when they're created.** A query that writes a component
  and also accesses it another way panics, naming the component (ADR-0010: a caller bug).
  Reading one component twice is allowed.
- **`World::has` is constant time:** each archetype keeps a component bitset (ADR-0008 §3.6).
- **Handles never alias within a world.** An index whose generation would wrap is retired,
  never reused. Handles carry **no world identity**, so a handle from another world isn't
  detected and may name an unrelated entity. Adding a world tag would change §3's bit layout,
  so it needs an ADR. Decide it before the editor holds two worlds (editor and PIE).
- **Notes for the next slices** (internal, so these change no API):
  - The access check is keyed by Rust type, and filters and resources declare nothing. The
    scheduler (§5) and dynamic queries (§7) need it keyed by component ID, with filter and
    resource access, and cached in a query state.
  - Typed Rust callers get panics for caller bugs: aliasing queries and duplicate bundles
    (ADR-0010). Dynamic and C-ABI paths build component sets from data, so they must
    **return errors** instead, because panics never cross FFI (TAD §5.8). Commands should check
    bundles when they're recorded, so a panic points at the call site and not at the sync
    point.
  - Component IDs are assigned per world, in first-use order. Copying a world for PIE, and
    C-ABI callers, need per-world resolution or a registry shared between worlds. Decide this
    with dynamic queries.
  - A component `Drop` that panics leaves a world inconsistent but memory-safe. This is
    documented on `Component`. Hooks will make re-entrancy likelier, so revisit it then.
- **Tests:**
  - behaviour tests for every public operation;
  - drop accounting: each component value is dropped exactly once;
  - a randomized 20,000-step comparison against a reference model.

  These deliberately broken versions each fail the tests:
  - a generation that isn't bumped (aliasing handles);
  - a moved entity whose location isn't updated;
  - the aliasing check turned off;
  - an inverted `Without`.
- **Benchmarks** live in the crate (`cargo bench -p oxreal-ecs`) rather than a top-level
  `benchmarks/ecs`, so they build and lint with the crate. Transform propagation joins them
  with relations.

  First results: AMD Ryzen 9 5950X, 64 GB, Windows 11, rustc 1.100.0, release build, PR #32's
  final commit, median of 11 runs, 100k entities.

  | Workload | Time |
  |---|---|
  | spawn (Position, Velocity) | 6.2 ms |
  | despawn | 1.0 ms |
  | iterate (&mut Position, &Velocity), 1 / 10 / 100 archetypes | 0.34 / 0.34 / 0.36 ms |
  | insert + remove a tag on 10k entities | 1.5 ms |
  | `get` through 100k shuffled handles | 3.3 ms |

  Iteration cost barely changes from 1 to 100 archetypes. If profiling points at the ECS, the
  first places to optimize are:
  - random access, at about 33 ns per lookup (a type-ID hash lookup on each call);
  - spawning;
  - the small allocation each query makes per archetype.

## Implementation notes (2026-11-20, second slice: systems, stages and commands)

- **Systems are explicit descriptors:** a name, declared reads and writes of components and
  resources, `before`/`after` constraints, and a closure (`System::new(..).reads::<T>()`).
  - They aren't Bevy-style functions whose parameter types imply their access. The descriptor
    is plain data that a C ABI can express (TAD §5.8). It also states access without generic
    machinery, and a parallel executor can read it (§5).
  - Touching undeclared data panics, naming the declaration to add. A declared write includes
    reading.
- **The system context is split into three fields:**
  - `ctx.world`: queries and lookups;
  - `ctx.resources`;
  - `ctx.commands`.

  Each borrows a different part of the world, so a system can iterate a query, read a resource
  and record commands at the same time with plain Rust borrows. Structure can't change while a
  system runs; only commands change it.
- **`Commands::spawn` returns a real handle at once.** `Entities` reserves it through a shared
  reference, by moving an atomic cursor over the free list, then into fresh indices. The free
  list only changes through `&mut`, after a flush turns reservations into reserved slots, so
  despawned slots are reused by command spawns too. This works unchanged under a parallel
  executor.
  - Every reservation queues the spawn that consumes it.
  - Reservations are lost only when a system panics: its stage's queue is discarded on the next
    run, with a warning. A lost reservation can't alias, because its slot is never reused.
- **Bundles are checked for duplicates when they're recorded** (`Commands::spawn`, `insert`), so
  the panic points at the call site.
- **Commands targeting gone entities:**
  - An insert into an entity that's gone by the sync point drops the bundle, with a warning
    naming the system.
  - Despawning and removing from a gone entity do nothing.
- **Ordering:**
  - Within a stage, systems are sorted topologically. Ties keep the order systems were added
    in, so runs are deterministic.
  - Constraints across stages are rejected (stages already order them).
  - A cycle is reported as the actual loop of system names, and nothing runs.
- **Deferred to later work:**
  - `DuringPhysics` runs once per `Schedule::run` until physics brings the fixed-step clock
    (D-10).
  - Events (§8) come with the gameplay framework.
- **The first consumer:** the cube example.
  - The camera, the spinning cube and the scenery are entities, and `InputState` and the frame
    time are resources.
  - Gameplay runs in `PrePhysics`, and an `Extract` system fills a draw list resource that the
    renderer reads (CLAUDE.md rule 3).
- **Tests** cover:
  - stage and tie order, constraints, and every schedule error, including the cycle report;
  - each undeclared-access panic;
  - command order, reserved handles and slot reuse;
  - the allocator's reservation arithmetic;
  - the cube scene's systems.

  These deliberately broken versions each fail the tests:
  - a wrong reserved slot;
  - a broken sort;
  - a read declaration that allows writes;
  - commands applied in reverse order.
