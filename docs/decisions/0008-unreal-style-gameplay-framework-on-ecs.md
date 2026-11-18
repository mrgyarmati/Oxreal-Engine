# ADR-0008: Unreal-style gameplay framework on top of the engine ECS

- **Status:** Proposed
- **Date:** 2026-11-18
- **Related:** PRD §6–§7, TAD §5.1, ADR-0005 (ECS and hierarchy), ADR-0006 (overrides), ADR-0007 (clean room); open decisions D-01, D-12, D-20

## Context

Unreal developers think in terms of Actors, Components, Pawns, Characters, Controllers and
possession, GameMode, GameState and PlayerState, subsystems, Blueprint classes and per-actor
`BeginPlay`/`Tick`. The PRD requires this to feel familiar. Our runtime model, however, is an
engine-owned ECS (ADR-0005), and Rust has no class inheritance.

Pure ECS removes a workflow Unreal users rely on: "this door has its own logic". An OOP actor
layer that owns the data would discard the ECS benefits: parallel scheduling, cache-friendly
iteration, data-oriented replication and simple serialization.

We take **concepts and terminology** from Unreal, not its implementation (ADR-0007). Types use
plain names (`Actor`, `Pawn`) without Unreal's `A`/`U`/`F` prefixes. Rust module paths handle
disambiguation.

## Options considered

### Option A: Pure ECS (no actor-level API)
- Pros: one model, maximum performance, simplest engine.
- Cons: unfamiliar to Unreal users. Per-object logic needs a dedicated system per behaviour,
  which is awkward for designers and visual scripting. It fails the familiarity requirement.

### Option B: OOP actor objects that own their data, with ECS only for internals
(similar to classic GameObject models)
- Pros: the most familiar model.
- Cons: two sources of truth, inheritance hierarchies that are hard to express in Rust, borrow
  conflicts between objects, and replication, serialization and the Details panel all needing a
  second path. It fights ADR-0005.

### Option C: Actor framework as a facade over the ECS (chosen)
- Pros: one source of truth (ECS components), with Unreal-familiar roles, names and per-actor
  behaviour layered on top. Performance-critical code stays as plain systems.
- Cons: two API levels to document. The ECS must support relations, lifecycle hooks and deferred
  commands. Per-actor behaviour scheduling needs care to avoid becoming a single-threaded bottleneck.

## Decision (proposed)

### 1. Terminology and mapping

| Unreal concept | Our concept | Representation |
|---|---|---|
| Object (`UObject`) | Reflected type / asset | Reflected Rust types (D-02), no GC. Lifetime by ownership and handles. |
| Actor (`AActor`) | **Actor** | An entity that exists in a Level, has a `Transform` and an authoring-hierarchy node, and is listed in the World Outliner. *Entity* stays the low-level runtime ID. Not every entity is an actor (internal, transient or render-only entities exist). |
| Actor component | **Component** | Plain data component plus systems that act on it. |
| Scene component / attachment | Child entity + hierarchy | Attachment is hierarchy parenting (ADR-0005). Sockets are named attach points. |
| Class inheritance (`ACharacter : APawn`) | **Composition / roles** | A role is a required set of components. `Character` = `Pawn` + `CapsuleCollider` + `CharacterMovement` (+ optional `SkeletalMesh`). Spawn helpers ("bundles") create a valid set. |
| `Cast<T>` / interfaces | Component queries / capabilities | "Is a Pawn?" means the entity has `Pawn`. Interfaces are marker components or reflection-registered traits (for example `Interactable`). |
| Blueprint class | **Actor template** (prefab) | An asset with components, default values, child entities and an optional Behaviour. Instances store overrides using the ADR-0006 override mechanism. |
| `BeginPlay` / `Tick` / `EndPlay` | **Behaviour** lifecycle | See §2. |
| Tick groups | **Schedule stages** | `PrePhysics`, `DuringPhysics`, `PostPhysics`, `PostUpdate` (transform propagation), `Extract` (render). Systems and behaviours declare their stage and ordering constraints. |
| Pawn / Controller / possession | **Relations** | `Possesses(Entity)` on the controller and `ControlledBy(Entity)` on the pawn, kept consistent by the possession API. `PlayerController` and `AiController` are roles. |
| GameMode | **GameMode** | A server-authoritative rules object (resource plus systems): spawning, rules, match flow. It never exists on clients. |
| GameState / PlayerState | **GameState / PlayerState** | Replicated entities: one GameState singleton, one PlayerState per player linked to its controller. |
| GameInstance / Subsystems | **Subsystems** | Resources with a lifecycle scope: `Engine`, `GameInstance`, `World`, `LocalPlayer`. |
| HUD | **Hud** | The runtime UI layer bound to a local player. |
| Weak object pointer | `Entity` handle | Generational. Stale handles fail lookups safely. |
| Replication | Reflected components with replication metadata | Authority, relevancy and RPCs are designed in the networking ADR. |

### 2. Two API levels, one data model

**Level 1: systems** (engine code, performance-critical gameplay). These are plain ECS systems
that iterate queries.

**Level 2: behaviours** (Actor-style, familiar to Unreal users). Per-actor logic is implemented
through a trait. A sketch, non-binding:

```rust
pub trait Behaviour: Reflect + Send + Sync + 'static {
    fn begin_play(&mut self, _actor: &mut ActorCtx<'_>) {}
    fn tick(&mut self, _actor: &mut ActorCtx<'_>, _dt: f32) {}
    fn end_play(&mut self, _actor: &mut ActorCtx<'_>, _reason: EndPlayReason) {}
    // Engine events are delivered as callbacks: overlap, hit, damage, possession, ...
}
```

- `ActorCtx` gives typed access to *this* actor's components, plus world access through
  **deferred commands** (spawn, destroy, send event) and read-only queries. That avoids aliasing
  `&mut` borrows across actors.
- Behaviours declare their tick stage and whether they tick at all (tick is opt-in, like
  disabling actor ticking in Unreal).
- Visual-script graphs (D-12) implement the same Behaviour contract, the way Blueprints sit over
  C++ classes.
- Initial execution model: behaviours of one stage run serially in a deterministic order.
  Parallel execution comes later, driven by declared component access, once profiling justifies it.

### 3. Requirements this places on the ECS (feeds D-01)

The ECS design must support:
1. Entity relations (possession, attachment, ownership) with consistency on despawn.
2. Lifecycle hooks or observers (on add/remove component, on spawn/despawn) to drive
   `begin_play`/`end_play`.
3. Deferred command buffers, applied at defined sync points.
4. Stages with ordering constraints (tick groups).
5. Reflection-driven component access (Details panel, templates, replication, scripting).
6. Cheap "has component" checks, which serve as role tests and casts.

### 4. Naming rules

- No `A`/`U`/`F`/`T`/`E`/`I` type prefixes. Use idiomatic Rust names (`Actor`, `Pawn`,
  `PlayerController`, `GameMode`).
- Use Unreal terminology in **user-facing** editor text and docs where it's the established term
  (World Outliner, Details, Play in Editor).

## Consequences

- Unreal developers get familiar concepts and per-actor logic while the data stays in one ECS.
- More API surface: Level 1 and Level 2 need to be documented separately with guidance on when
  to use which.
- The ECS work in D-01 now has concrete requirements (§3), so it should be benchmarked with
  behaviour-heavy workloads as well as bulk iteration.
- Follow-up ADRs are needed for networking/replication, the visual scripting IR (D-12) and
  Rust gameplay hot reload (D-11).
- Revisit triggers: behaviour execution dominating frame time in benchmark projects, or user
  testing showing that the template/role model confuses Unreal developers.
